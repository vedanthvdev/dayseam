// IPC types shared between the Rust core and the TypeScript frontend.
// These are hand-written in the scaffold phase; a later Phase 1 task wires
// up `ts-rs` generation from Rust so renames surface as TS compile errors.

export type PlaceholderIpcType = {
  readonly kind: "placeholder";
};
