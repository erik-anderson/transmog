import js from "@eslint/js";
import stylistic from "@stylistic/eslint-plugin";
import { defineConfig, globalIgnores } from "eslint/config";
import prettier from "eslint-config-prettier/flat";
import tseslint from "typescript-eslint";

export default defineConfig(
  // Keep generated build output and vendored sources out of repository checks.
  globalIgnores([
    "**/node_modules/**",
    "**/dist/**",
    "**/target/**",
    "**/bin/**",
    "**/obj/**",
    ".tools/**",
    ".local/**",
    "artifacts/**",
    "third-party/**",
    "third_party/**",
    "apps/desktop/gen/**",
    "e2e/playwright/test-results/**",
    "e2e/playwright/playwright-report/**",
  ]),
  {
    files: ["**/*.{ts,tsx,mts,cts}"],
    extends: [js.configs.recommended, tseslint.configs.recommended, prettier],
    plugins: { "@stylistic": stylistic },
    linterOptions: { reportUnusedDisableDirectives: "error" },
    rules: {
      "@stylistic/max-statements-per-line": ["error", { max: 1 }],
      "@typescript-eslint/consistent-type-imports": "error",
      "@typescript-eslint/no-unused-vars": [
        "error",
        {
          argsIgnorePattern: "^_",
          varsIgnorePattern: "^_",
          caughtErrorsIgnorePattern: "^_",
          ignoreRestSiblings: true,
        },
      ],
      curly: ["error", "all"],
      eqeqeq: ["error", "always", { null: "ignore" }],
      "no-var": "error",
      "prefer-const": "error",
    },
  },
);
