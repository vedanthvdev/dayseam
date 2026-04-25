// Barrel file for the IPC module. Components should import from
// `../ipc` rather than reaching into individual files, so the module
// boundary stays narrow.

export { invoke, Channel } from "./invoke";
export { useToasts, TOAST_EVENT } from "./useToasts";
export type { QueuedToast } from "./useToasts";
export { useLogsTail } from "./useLogsTail";
export type { UseLogsTailOptions, UseLogsTailState } from "./useLogsTail";
export { useSources } from "./useSources";
export type { UseSourcesState } from "./useSources";
export { useIdentities } from "./useIdentities";
export type { UseIdentitiesState } from "./useIdentities";
export { useLocalRepos } from "./useLocalRepos";
export type { UseLocalReposState } from "./useLocalRepos";
export { useSinks } from "./useSinks";
export type { UseSinksState } from "./useSinks";
export { useSettings } from "./useSettings";
export type { UseSettingsState } from "./useSettings";
export { useReport, REPORT_COMPLETED_EVENT } from "./useReport";
export type { UseReportState, ReportState, ReportStatus } from "./useReport";
