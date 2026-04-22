@github @connector:github
Feature: Connect a GitHub source
  As a Dayseam user who tracks work in GitHub (cloud or Enterprise),
  I want to connect my account through a single Personal Access Token,
  so the daily report can surface my GitHub activity alongside my repos.

  Background:
    Given the Dayseam desktop app is open on the main screen

  # DAY-99. Drives the real `AddGithubSourceDialog`:
  #   1. open from the sidebar "+ Add source" menu,
  #   2. paste the fixture PAT against the pre-filled cloud URL,
  #   3. click Validate — the mock's
  #      `github_validate_credentials` returns the fixture triple
  #      and the dialog renders the "✓ Connected as …" ribbon,
  #   4. click Add source — the mock's `github_sources_add`
  #      persists the row and the dialog closes.
  # The final Then assertion pins the IPC payload the renderer
  # sent so the normalisation + userId plumbing is covered
  # end-to-end, not just in the RTL unit suite.
  @smoke @github-happy-path
  Scenario: Add a GitHub cloud source through the dialog
    When I open the Add GitHub source dialog
    Then the GitHub API base URL hint shows the normalised URL

    When I fill the GitHub credentials from the fixture
    And I validate the GitHub credentials
    And I confirm the Add GitHub dialog

    Then the captured GitHub add-source IPC matches the fixture
    And no console or page errors were captured during the run
