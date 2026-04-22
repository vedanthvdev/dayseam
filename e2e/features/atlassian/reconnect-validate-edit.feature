@atlassian @connector:atlassian @validate-edit
Feature: Re-validate an Atlassian token after editing credentials
  As a Dayseam user who's mid-way through connecting Atlassian,
  I want the dialog to force a fresh Validate after I edit the URL or email,
  so I can't accidentally bind a source to whatever account the previous
  Validate probe returned.

  Background:
    Given the Dayseam desktop app is open on the main screen

  # DAY-90 TST-v0.2-05. The original dialog cached the result of
  # the first `atlassian_validate_credentials` probe and — since the
  # Add button only checked `validation.kind === "ok"` — a user who
  # validated against one workspace and then edited the URL to point
  # at another could persist a `SourceIdentity` whose `account_id`
  # came from the first workspace. The dialog now invalidates the
  # cached status on every credential-input change; this scenario
  # proves the invalidation is user-visible and the second Validate
  # click is actually required (not a no-op reusing the first result).
  @smoke
  Scenario: Editing the email after Validate forces a re-validation before Add source
    When I open the Add Atlassian source dialog
    And I select only the Jira product
    And I fill the Atlassian credentials from the fixture

    When I validate the Atlassian credentials
    And I edit the Atlassian email and expect the validation to clear

    When I validate the Atlassian credentials
    And I confirm the Add Atlassian dialog
    And I generate a report

    Then the streaming preview shows the completed draft
    And the draft contains the Atlassian Jira bullet
    And no console or page errors were captured during the run
