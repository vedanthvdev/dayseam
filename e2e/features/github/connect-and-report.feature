@github @connector:github
Feature: Connect a GitHub source and include it in a daily report
  As a Dayseam user who tracks work in GitHub (cloud or Enterprise),
  I want to connect my account through a single Personal Access Token,
  so my daily report picks up the activity I recorded there alongside my repos.

  Background:
    Given the Dayseam desktop app is open on the main screen

  # DAY-100. Parallels `atlassian/connect-and-report.feature`:
  # drives the real `AddGithubSourceDialog` end-to-end, then
  # clicks `Generate report` and asserts the draft carries the
  # deterministic GitHub bullet the mock's `report_get` appends
  # whenever a GitHub source is present. Complements the existing
  # `add-source.feature` (which pins the IPC payload) by pinning
  # the *report-side* consequence of adding that source — the
  # matched-pair regression that would slip past either test in
  # isolation.
  @smoke @github-happy-path
  Scenario: Connect a GitHub source, then generate a report with a GitHub bullet
    When I open the Add GitHub source dialog
    Then the GitHub API base URL hint shows the normalised URL

    When I fill the GitHub credentials from the fixture
    And I validate the GitHub credentials
    And I confirm the Add GitHub dialog
    And I generate a report
    Then the streaming preview shows the completed draft
    And the draft contains the GitHub pull request bullet
    And no console or page errors were captured during the run
