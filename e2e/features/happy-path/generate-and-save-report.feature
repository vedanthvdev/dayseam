@happy-path @smoke
Feature: Generate a daily report and save it to a markdown sink
  As a Dayseam user with my sources and sink already configured,
  I want to generate a daily report and save it to my notes folder,
  so I can keep a recorded receipt of what I shipped today.

  Background:
    Given the Dayseam desktop app is open on the main screen

  @save-ipc-contract
  Scenario: Happy path — generate, save, and confirm the receipt
    When I generate a report
    Then the streaming preview shows the completed draft
    And the "completed" section contains 2 bullets
    And the "completed" section contains the bullet "Wired up the Playwright E2E happy path"

    When I save the draft to the configured markdown sink
    Then a save receipt is shown listing "/tmp/dayseam-e2e-sink/daily-note.md"
    And the captured save IPC call targets the configured sink at "/tmp/dayseam-e2e-sink/daily-note.md"
    And no console or page errors were captured during the run
