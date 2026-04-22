@github @connector:github @validate-edit
Feature: Re-validate a GitHub PAT after editing credentials
  As a Dayseam user who's mid-way through connecting GitHub,
  I want the dialog to force a fresh Validate after I edit the PAT,
  so I can't accidentally bind a source to whatever account the
  previous Validate probe returned.

  Background:
    Given the Dayseam desktop app is open on the main screen

  # DAY-99. `AddGithubSourceDialog` caches the result of
  # `github_validate_credentials` in component state and gates the
  # Add-source button on `validation.kind === "ok"`. If the user
  # edits the PAT (or the API base URL) after a successful probe,
  # the cached `ok` ribbon must drop — otherwise a user who
  # validated with one token and then pasted another could persist
  # a `SourceIdentity` whose numeric `user_id` came from the first
  # probe. This scenario proves the invalidation is user-visible
  # and the second Validate click is actually required (not a
  # no-op reusing the first result).
  @smoke
  Scenario: Editing the PAT after Validate forces a re-validation before Add source
    When I open the Add GitHub source dialog
    And I fill the GitHub credentials from the fixture

    When I validate the GitHub credentials
    And I edit the GitHub PAT and expect the validation to clear

    When I validate the GitHub credentials
    And I confirm the Add GitHub dialog

    Then the captured GitHub add-source IPC matches the fixture
    And no console or page errors were captured during the run
