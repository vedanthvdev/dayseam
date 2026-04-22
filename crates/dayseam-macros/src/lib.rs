//! Compile-time helpers for Dayseam.
//!
//! # What lives here
//!
//! Exactly one public item today: the [`SerdeDefaultAudit`] derive
//! macro. It is a structural lint targeting the class of bug DAY-88
//! spent a full day chasing (`DOG-v0.2-04`): a field marked
//! `#[serde(default)]` on a persisted struct deserialises cleanly
//! from a stale on-disk payload but then fails downstream validation,
//! turning into a silent data-loss or UX paper-cut whose cause is far
//! from its symptom.
//!
//! The derive forces every such field to carry one of two explicit
//! attributes:
//!
//! * `#[serde_default_audit(repair = "…")]` — names the registered
//!   [`SerdeDefaultRepair`] impl responsible for patching old rows
//!   at boot. The name must match the repair's `name()` method; the
//!   cross-check happens in a paired runtime test
//!   ([`dayseam_db::repairs::tests::registered_repairs_has_no_duplicate_names`]
//!   already pins repair-name uniqueness), not in the macro itself —
//!   a proc macro cannot see the contents of another crate.
//! * `#[serde_default_audit(no_repair = "…")]` — an explicit waiver.
//!   The string argument is required and must be non-empty; the
//!   review-time justification becomes a mandatory artefact in the
//!   PR diff instead of a quiet `#[serde(default)]` slipping past
//!   review unnoticed.
//!
//! # Scope
//!
//! * The macro inspects the direct fields of the struct (or enum
//!   variant) it decorates. It does **not** recurse into nested types
//!   — each nested type is expected to carry its own
//!   `#[derive(SerdeDefaultAudit)]` attribute if its fields need the
//!   same enforcement.
//! * Enum inputs are supported; every variant's fields are audited as
//!   if they were a struct (this matches the shape of
//!   `dayseam_core::SourceConfig`, which is an enum with struct-like
//!   variants).
//! * Named-field and tuple-field structs are both supported. Unit
//!   structs / unit variants have no fields so the derive is a no-op
//!   on them.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, Attribute, DeriveInput, Fields, LitStr, Meta};

/// Derive macro that audits every `#[serde(default)]` field on the
/// decorated type and fails to compile unless each such field also
/// carries `#[serde_default_audit(repair = "…")]` or
/// `#[serde_default_audit(no_repair = "reason")]`.
///
/// See the crate-level docs for the rationale and the exact grammar.
#[proc_macro_derive(SerdeDefaultAudit, attributes(serde_default_audit))]
pub fn derive_serde_default_audit(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);

    let mut errors: Vec<syn::Error> = Vec::new();

    match &ast.data {
        syn::Data::Struct(s) => audit_fields(&s.fields, &ast.ident.to_string(), &mut errors),
        syn::Data::Enum(e) => {
            for variant in &e.variants {
                let ctx = format!("{}::{}", ast.ident, variant.ident);
                audit_fields(&variant.fields, &ctx, &mut errors);
            }
        }
        // Unions have no fields in the serde sense and are not
        // #[derive(Serialize)]-compatible anyway; accept without
        // complaint so a future type that happens to derive the audit
        // on a union (accidentally) reports the real serde error, not
        // a spurious one from us.
        syn::Data::Union(_) => {}
    }

    let ty = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    // Emit a marker impl so the derive has an observable effect even
    // on types that audit cleanly — without a generated item the
    // derive becomes invisible to documentation and to downstream
    // tooling (e.g. rust-analyzer's "expand macro"). The impl is a
    // zero-sized sentinel; all it does is pin the auditor's presence.
    let marker_name = syn::Ident::new(
        &format!("__DayseamSerdeDefaultAuditMarker_{ty}"),
        Span::call_site(),
    );
    let marker = quote! {
        #[doc(hidden)]
        #[allow(non_camel_case_types, dead_code)]
        struct #marker_name;
        #[doc(hidden)]
        impl #impl_generics #ty #ty_generics #where_clause {
            #[allow(non_upper_case_globals, dead_code)]
            const __DAYSEAM_SERDE_DEFAULT_AUDITED: () = ();
        }
    };

    if errors.is_empty() {
        marker.into()
    } else {
        let compile_errors = errors
            .into_iter()
            .map(|e| e.to_compile_error())
            .collect::<Vec<_>>();
        quote! {
            #(#compile_errors)*
            #marker
        }
        .into()
    }
}

// ---- Internals ------------------------------------------------------------

/// Walk every field in `fields`, and for each field carrying a
/// `#[serde(default)]` attribute (including
/// `#[serde(default = "path")]`) require a matching
/// `#[serde_default_audit(...)]` that justifies it.
fn audit_fields(fields: &Fields, ctx: &str, errors: &mut Vec<syn::Error>) {
    let iter: Box<dyn Iterator<Item = &syn::Field>> = match fields {
        Fields::Named(n) => Box::new(n.named.iter()),
        Fields::Unnamed(u) => Box::new(u.unnamed.iter()),
        Fields::Unit => Box::new(std::iter::empty()),
    };

    for (idx, field) in iter.enumerate() {
        if !has_serde_default(&field.attrs) {
            continue;
        }
        match parse_audit_decision(&field.attrs) {
            Ok(Some(_)) => {
                // Decision explicitly recorded; field passes.
            }
            Ok(None) => {
                let name = field
                    .ident
                    .as_ref()
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| format!(".{idx}"));
                errors.push(syn::Error::new_spanned(
                    field,
                    format!(
                        "`{ctx}.{name}` is `#[serde(default)]` but carries no \
                         `#[serde_default_audit(...)]` annotation. \
                         Add either `#[serde_default_audit(repair = \"NAME\")]` \
                         (naming a registered `SerdeDefaultRepair` impl) or \
                         `#[serde_default_audit(no_repair = \"reason\")]` \
                         to document the deliberate waiver. \
                         Background: DOG-v0.2-04 / DAY-88 silent-failure sweep.",
                    ),
                ));
            }
            Err(e) => errors.push(e),
        }
    }
}

/// Return `true` if `attrs` carries any `#[serde(default)]` or
/// `#[serde(default = "…")]` tag. Tolerates the serde convention of
/// stacking unrelated keys in the same attribute (`#[serde(rename =
/// "x", default)]`); the match only cares that `default` appears.
fn has_serde_default(attrs: &[Attribute]) -> bool {
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let mut found = false;
        // `parse_nested_meta` mirrors how serde itself parses its
        // attribute grammar, so we match the same surface exactly.
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("default") {
                found = true;
                // Swallow any `= "path"` without complaint; we are
                // only looking for presence.
                if meta.input.peek(syn::Token![=]) {
                    let _: syn::Expr = meta.value()?.parse()?;
                }
            } else if meta.input.peek(syn::Token![=]) {
                // Any other key with a value: skip its value so the
                // parser can continue onto subsequent keys.
                let _: syn::Expr = meta.value()?.parse()?;
            }
            Ok(())
        });
        if found {
            return true;
        }
    }
    false
}

/// Output of a successful `#[serde_default_audit(...)]` parse. The
/// derive only cares that one of these was supplied; the variant
/// distinction is kept so a future extension (e.g. emitting a runtime
/// assertion for the `Repair` case) has somewhere to hook.
enum AuditDecision {
    Repair(#[allow(dead_code)] String),
    NoRepair(#[allow(dead_code)] String),
}

/// Parse the first `#[serde_default_audit(...)]` attribute on a field.
/// `Ok(None)` means no such attribute was present (the audit-failure
/// case); `Ok(Some(_))` means a well-formed decision; `Err(_)` means
/// the attribute was malformed (e.g. `no_repair = ""`) and should
/// surface as its own compile error.
fn parse_audit_decision(attrs: &[Attribute]) -> Result<Option<AuditDecision>, syn::Error> {
    for attr in attrs {
        if !attr.path().is_ident("serde_default_audit") {
            continue;
        }
        let args: AuditArgs = attr.parse_args()?;
        let decision = match args {
            AuditArgs::Repair(name) => {
                if name.value().trim().is_empty() {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "`serde_default_audit(repair = \"…\")` requires a \
                         non-empty repair name that matches a registered \
                         `SerdeDefaultRepair::name()`",
                    ));
                }
                AuditDecision::Repair(name.value())
            }
            AuditArgs::NoRepair(reason) => {
                if reason.value().trim().is_empty() {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "`serde_default_audit(no_repair = \"…\")` requires a \
                         non-empty justification. The review-time rationale is \
                         the whole point of the waiver — a blank string defeats \
                         the audit.",
                    ));
                }
                AuditDecision::NoRepair(reason.value())
            }
        };
        return Ok(Some(decision));
    }
    Ok(None)
}

enum AuditArgs {
    Repair(LitStr),
    NoRepair(LitStr),
}

impl Parse for AuditArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let meta: Meta = input.parse()?;
        let nv = match meta {
            Meta::NameValue(nv) => nv,
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "expected `serde_default_audit(repair = \"…\")` or \
                     `serde_default_audit(no_repair = \"…\")`",
                ));
            }
        };
        let key = nv
            .path
            .get_ident()
            .map(|i| i.to_string())
            .unwrap_or_default();
        let lit = expect_str_lit(&nv.value)?;
        match key.as_str() {
            "repair" => Ok(AuditArgs::Repair(lit)),
            "no_repair" => Ok(AuditArgs::NoRepair(lit)),
            _ => Err(syn::Error::new_spanned(
                &nv.path,
                "unknown `serde_default_audit` key; expected `repair` or `no_repair`",
            )),
        }
    }
}

fn expect_str_lit(expr: &syn::Expr) -> syn::Result<LitStr> {
    if let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = expr
    {
        Ok(s.clone())
    } else {
        Err(syn::Error::new_spanned(
            expr,
            "expected a string literal, e.g. `\"confluence_email\"`",
        ))
    }
}
