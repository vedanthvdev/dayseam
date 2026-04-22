// Single entry point a step definition uses to reach every page
// object. `pages.report.clickGenerate()` reads naturally in a step
// body and keeps each page object's constructor out of the step
// code. Modulr's customer-portal-v2 suite uses the same factory
// pattern; we keep the shape familiar while only instantiating the
// pages this repo actually needs.

import type { Page } from "@playwright/test";
import { AppShellPage } from "../../page-objects/app-shell/app-shell-page";
import { AtlassianDialogPage } from "../../page-objects/atlassian/atlassian-dialog-page";
import { GithubDialogPage } from "../../page-objects/github/github-dialog-page";
import { ReportPage } from "../../page-objects/report/report-page";
import { SaveDialogPage } from "../../page-objects/save/save-dialog-page";

export class PageFactory {
  readonly appShell: AppShellPage;
  readonly atlassian: AtlassianDialogPage;
  readonly github: GithubDialogPage;
  readonly report: ReportPage;
  readonly save: SaveDialogPage;

  constructor(page: Page) {
    this.appShell = new AppShellPage(page);
    this.atlassian = new AtlassianDialogPage(page);
    this.github = new GithubDialogPage(page);
    this.report = new ReportPage(page);
    this.save = new SaveDialogPage(page);
  }
}
