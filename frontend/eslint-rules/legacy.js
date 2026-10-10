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
  "jsx-a11y/aria-role": [
    "src/routes/control-plane/StarRocksConfigTab.tsx",
    "src/routes/control-plane/config/CoordinatorSection.tsx",
    "src/routes/control-plane/config/WorkerSection.tsx",
  ],
  "jsx-a11y/click-events-have-key-events": [
    "src/components/chart/ObjectDetail.tsx",
    "src/components/chart/dashboard/CardInlineFilters.tsx",
    "src/components/chart/dashboard/FilterPill.tsx",
    "src/components/ui/tag-combobox.tsx",
  ],
  "jsx-a11y/heading-has-content": [
    "src/components/ui/alert.tsx",
    "src/components/ui/card.tsx",
  ],
  "jsx-a11y/label-has-associated-control": [
    "src/components/chart/dashboard/CardSettingsPanel.tsx",
    "src/components/chart/dashboard/DashboardFilterBar.tsx",
    "src/components/chart/settings/BoxplotDisplaySection.tsx",
    "src/components/chart/settings/CartesianDisplaySection.tsx",
    "src/components/chart/settings/RuleEditor.tsx",
    "src/components/chart/settings/WaterfallDisplaySection.tsx",
    "src/routes/control-plane/PodLogsDialog.tsx",
    "src/routes/explore/DatasetDetail.tsx",
    "src/routes/explore/settings/RolesPermissions.tsx",
    "src/routes/governance/GrantDialog.tsx",
  ],
  "jsx-a11y/no-noninteractive-element-interactions": [
    "src/components/chart/ObjectDetail.tsx",
    "src/components/chart/TableHeaderControls.tsx",
    "src/components/chart/dashboard/FilterPill.tsx",
  ],
  "jsx-a11y/no-noninteractive-tabindex": [
    "src/components/access/MembersPanel.tsx",
    "src/components/chart/TableHeaderControls.tsx",
  ],
  "jsx-a11y/no-static-element-interactions": [
    "src/components/chart/dashboard/CardInlineFilters.tsx",
  ],
  "jsx-a11y/role-has-required-aria-props": [
    "src/components/chart/settings/CurrencySelect.tsx",
  ],
};
