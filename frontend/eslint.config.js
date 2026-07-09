import js from '@eslint/js';
import globals from 'globals';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';
import tseslint from 'typescript-eslint';

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
);
