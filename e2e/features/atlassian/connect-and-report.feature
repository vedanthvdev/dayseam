@atlassian @connector:atlassian
Feature: Connect Atlassian sources and include them in a daily report
  As a Dayseam user who tracks work in Jira and/or Confluence,
  I want to connect those products through a single Atlassian API token,
  so my daily report picks up the activity I recorded there alongside my repos.

  Background:
    Given the Dayseam desktop app is open on the main screen

  @smoke @atlassian-jira-only
  Scenario: Connect Jira only, then generate a report with a Jira bullet
    When I open the Add Atlassian source dialog
    And I select only the Jira product
    And I fill the Atlassian credentials from the fixture
    Then the workspace URL hint shows the normalised origin

    When I validate the Atlassian credentials
    And I confirm the Add Atlassian dialog
    And I generate a report
    Then the streaming preview shows the completed draft
    And the draft contains the Atlassian Jira bullet
    And no console or page errors were captured during the run

  @smoke @atlassian-confluence-only
  Scenario: Connect Confluence only, then generate a report with a Confluence bullet
    When I open the Add Atlassian source dialog
    And I select only the Confluence product
    And I fill the Atlassian credentials from the fixture
    And I validate the Atlassian credentials
    And I confirm the Add Atlassian dialog
    And I generate a report
    Then the streaming preview shows the completed draft
    And the draft contains the Atlassian Confluence bullet
    And no console or page errors were captured during the run

  @smoke @atlassian-both
  Scenario: Connect Jira and Confluence in one flow, then generate a grouped report
    When I open the Add Atlassian source dialog
    And I select both Atlassian products
    And I fill the Atlassian credentials from the fixture
    And I validate the Atlassian credentials
    And I confirm the Add Atlassian dialog
    And I generate a report
    Then the streaming preview shows the completed draft
    And the draft shows a Jira and a Confluence bullet in the Completed section
    And no console or page errors were captured during the run
