# User Issue Ledger: UI / scope

| ID | Applies to | Mistake pattern | Required behavior | Prevention and verification |
| --- | --- | --- | --- | --- |
| UIL-UI-SCOPE-001 | UI requests made from the DevCoordinator2 repository, especially references to the Console | Interpreting an unqualified request for the “console” as the Codex terminal UI moved discovery into another repository and proposed the wrong product surface | Treat the current DevCoordinator2 web Console as the target unless the user explicitly names another product or repository; external products may be read as data sources but remain unchanged without explicit scope | Before planning UI work, confirm the current repository and inspect its existing Console routes and visual language; record any external dependency as read-only, and verify the final diff contains no out-of-scope repository changes |
