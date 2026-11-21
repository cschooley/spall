import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";

// Captured traffic is attacker-controlled. If it ever renders as HTML, the
// target app gets XSS against the tester. These bans are errors; an
// exception needs an eslint-disable comment with a reason, and review.
// (docs/architecture.md, Frontend: "Untrusted content")
export const untrustedContentRules = {
  // Equivalent of react/no-danger, without eslint-plugin-react.
  "no-restricted-syntax": [
    "error",
    {
      selector: "JSXAttribute[name.name='dangerouslySetInnerHTML']",
      message: "Traffic is never HTML: dangerouslySetInnerHTML is banned.",
    },
  ],
  "no-restricted-properties": [
    "error",
    ...["innerHTML", "outerHTML", "insertAdjacentHTML"].map((property) => ({
      property,
      message: `Traffic is never HTML: ${property} is banned. Use textContent or JSX.`,
    })),
  ],
};

export default tseslint.config(
  { ignores: ["dist", "node_modules"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: { globals: globals.browser },
    plugins: { "react-hooks": reactHooks },
    rules: {
      ...reactHooks.configs.recommended.rules,
      ...untrustedContentRules,
    },
  },
  {
    files: ["scripts/**/*.mjs", "eslint.config.js"],
    languageOptions: { globals: globals.node },
  },
);
