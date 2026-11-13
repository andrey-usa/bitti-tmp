# Gherkin

`.feature` files run as they are, beside `.bt` files (no generated code):
`Feature`, `Background` (before each scenario), `Scenario`/`Example`,
`Scenario Outline` (or `Scenario Template`) with `Examples` (or
`Scenarios`) tables, several allowed, each with its own tags (`<column>`
filled in; one scenario per row, named `Name (col=value, …)` when the name
has no `<…>`), `Rule` (a `Background` under it runs before that rule's
scenarios only), tags (a feature's go on all its scenarios), and
`Given`/`When`/`Then`/`And`/`But`/`*` steps, all alike. English keywords
only.

A step is one of the run's own `step` blocks, from any `.bt` file it loads:

```text
step "I sign in with {password}"
  fill --label Password ${password}
  click --text "Sign in"
```

`{name}` takes a `"…"` or `'…'` value or one word and is `${name}` in the
steps under it. A line two `step` blocks match is an error. Else it is a
phrase of the built-in book: `I open "/cart"` (`go to`, `navigate to`,
`visit`, `am on`), `I click "Apply"` (`click on`; by visible text), `I fill
"Email" with "a@b.c"` (`fill in`, `enter`; by label), `I type "x" into
"Search"`, `I select "Canada" from "Country"` (or `in`), `I check "Terms"`,
`I uncheck …`, `I press "Enter"`, `I wait for "Saved"`, `I see "Saved"` (`I
should see`), `I don't see "Error"` (`should not`, `shouldn't`, `do not`),
`the URL is|contains|starts with|ends with|matches "…"` (`url`, `page URL`),
`the title is|contains … "…"` (`page title`). An icon button or a field
with no label needs a `step` block (`click --role …`, `--placeholder`). A
step nothing matches is a file error that says how to write it. Data tables and doc strings under a
step aren't supported yet.

`--cucumber <file>` writes Cucumber JSON for the `.feature` files, step by
step (passed, failed, skipped, with times), for test-management importers.
`--junit`, `--html`, `--allure` and the console report them like any
scenario.
