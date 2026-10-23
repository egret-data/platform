import js from '@eslint/js';
import globals from 'globals';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';
import tseslint from 'typescript-eslint';
import egret from './eslint-rules/index.js';
import jsxA11yX from 'eslint-plugin-jsx-a11y-x';
import { plugins, a11ySettings, a11yRules } from './eslint-rules/plugins.js';
import { legacy } from './eslint-rules/legacy.js';

const EGRET_FILES = ['src/**/*.{ts,tsx}'];
const TESTS = ['**/__tests__/**', '**/*.test.{ts,tsx}'];

export default tseslint.config(
  // `src/api/generated` is orval-emitted build output — not hand-maintained,
  // so it's exempt from lint. All hand-written API code (mutator, the thin
  // per-domain adapter modules) lives outside this dir and is linted normally.
  { ignores: ['dist', 'node_modules', 'public', 'src/api/generated'] },
  {
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    files: ['**/*.{ts,tsx}'],
    languageOptions: {
      ecmaVersion: 2020,
      globals: globals.browser,
    },
    plugins: {
      'react-hooks': reactHooks,
      'react-refresh': reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // eslint-plugin-react-hooks 7 folded the React Compiler's static analysis into
      // `recommended`, and those rules land as errors on code written years before
      // them: 63 set-state-in-effect, 46 refs, and a handful more. Fixing them means
      // restructuring effects across the app — real work, but product work, not part
      // of moving the linter to ESLint 10. They are warnings so the backlog is
      // visible and CI stays honest about what actually regressed; drop each one back
      // to its default as its call sites are cleaned up.
      //
      // `rules-of-hooks` deliberately stays an error. It fires exactly twice, both
      // false positives, and both carry a targeted disable naming the reason.
      'react-hooks/set-state-in-effect': 'warn',
      'react-hooks/refs': 'warn',
      'react-hooks/immutability': 'warn',
      'react-hooks/static-components': 'warn',
      'react-hooks/use-memo': 'warn',
      'react-hooks/purity': 'warn',
      // Joined the list when ChartTable moved to react-table 9: the compiler stops
      // being able to prove the `sortedPosition` memo is preservable once the row
      // model comes from v9's table object. Same character as the rules above — it
      // reports a missed optimisation, not a defect.
      'react-hooks/preserve-manual-memoization': 'warn',
      // react-refresh: conflicts with shadcn/cva pattern that exports
      // both a component and its variants helper from the same file.
      // Off across the board since Egret uses shadcn extensively.
      'react-refresh/only-export-components': 'off',
      // no-explicit-any: off to keep CI signal high; revisit per-file later.
      '@typescript-eslint/no-explicit-any': 'off',
      '@typescript-eslint/no-unused-vars': [
        'warn',
        { argsIgnorePattern: '^_', varsIgnorePattern: '^_' },
      ],
    },
  },
  {
    files: EGRET_FILES,
    ignores: [...TESTS, 'src/components/chart/echarts/**'],
    plugins: { egret },
    rules: { 'egret/no-raw-palette': 'error' },
  },
  {
    files: EGRET_FILES,
    ignores: TESTS,
    plugins: { egret },
    rules: {
      'egret/no-arbitrary-text-size': 'error',
      'egret/no-window-confirm': 'error',
      'egret/no-native-select': 'error',
    },
  },
  {
    files: ['src/**/*.tsx'],
    ignores: TESTS,
    plugins: { 'jsx-a11y': plugins['jsx-a11y'] },
    languageOptions: jsxA11yX.configs.recommended.languageOptions,
    settings: a11ySettings,
    rules: a11yRules,
  },
  // The legacy lists: each rule off for exactly the files that broke it when it
  // was introduced (docs/en/frontend-rules.md, the debt register).
  ...Object.entries(legacy).map(([rule, files]) => ({ files, rules: { [rule]: 'off' } })),
);
