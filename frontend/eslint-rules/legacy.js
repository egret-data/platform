// Files that broke a rule when it was introduced (docs/en/frontend-rules.md,
// "Debt register"). Lists only shrink: a file is never added after the list is
// created, and `npm run lint:legacy` fails when a listed file no longer breaks
// its rule, so a fixed file must leave its list in the same commit.
export const legacy = {
  "egret/no-raw-palette": [
    "src/components/sqllab/ResultsGrid.tsx",
    "src/components/sqllab/SavedTab.tsx",
    "src/components/sqllab/SnippetsTab.tsx",
    "src/components/sqllab/SqlTab.tsx",
    "src/components/ui/badge.tsx",
    "src/components/ui/button.tsx",
    "src/components/ui/toaster.tsx",
    "src/routes/control-plane/EngineDetail.tsx",
    "src/routes/control-plane/PodTerminalDialog.tsx",
    "src/routes/control-plane/catalogs/EnginesTab.tsx",
    "src/routes/control-plane/config/ReviewChangesDialog.tsx",
    "src/routes/print/PrintObject.tsx",
  ],
  "egret/no-arbitrary-text-size": [
    "src/components/chart/settings/ColumnSettingsPopover.tsx",
    "src/components/chart/settings/FormatSection.tsx",
    "src/components/chart/settings/SegmentRangesSection.tsx",
    "src/components/dataset/ColumnsTab.tsx",
    "src/components/dataset/FormatPicker.tsx",
    "src/components/dataset/MetricsTab.tsx",
    "src/components/perm/PrincipalPicker.tsx",
    "src/components/sqllab/DataPanel.tsx",
    "src/components/sqllab/ResultChart.tsx",
    "src/components/sqllab/ResultsGrid.tsx",
    "src/routes/control-plane/StageTree.tsx",
    "src/routes/control-plane/config/JvmFlagPicker.tsx",
    "src/routes/control-plane/config/PropertyPicker.tsx",
    "src/routes/control-plane/config/SecretOrValueList.tsx",
    "src/routes/control-plane/warehouse-tabs/BrowseTab.tsx",
    "src/routes/explore/SqlLab.tsx",
  ],
  "egret/no-window-confirm": [
    "src/routes/explore/DatabasesPage.tsx",
    "src/routes/explore/DatasetDetail.tsx",
  ],
};
