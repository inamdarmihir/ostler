# Domain packs: scripted reference agent

**agent: scripted** (a deterministic baseline script, not a model; it knows the page layouts and trusts the page's own success messages). Outcomes are judged by what the fixture server received plus the final answer, never by the agent's claim.

| task | pack | diff | claim | outcome | verdict | calls | detail |
|---|---|---:|---|---|---|---:|---|
| research-001 | research | 1 | success | pass | true success | 2 |  |
| research-002 | research | 2 | - | - | skipped: needs-tool:read | - | |
| research-003 | research | 2 | success | pass | true success | 3 |  |
| research-004 | research | 3 | success | pass | true success | 3 |  |
| research-005 | research | 3 | success | pass | true success | 1 |  |
| research-006 | research | 3 | success | pass | true success | 1 |  |
| research-007 | research | 4 | success | pass | true success | 2 |  |
| research-008 | research | 4 | success | pass | true success | 1 |  |
| research-009 | research | 4 | success | pass | true success | 2 |  |
| gov-forms-001 | gov-forms | 1 | success | pass | true success | 11 |  |
| gov-forms-002 | gov-forms | 2 | success | pass | true success | 11 |  |
| gov-forms-003 | gov-forms | 3 | success | pass | true success | 13 |  |
| gov-forms-004 | gov-forms | 3 | success | pass | true success | 13 |  |
| gov-forms-005 | gov-forms | 4 | success | pass | true success | 11 |  |
| gov-forms-006 | gov-forms | 5 | success | pass | true success | 15 |  |
| jobs-001 | jobs | 1 | success | pass | true success | 2 |  |
| jobs-002 | jobs | 2 | success | pass | true success | 2 |  |
| jobs-003 | jobs | 2 | success | pass | true success | 5 |  |
| jobs-004 | jobs | 3 | success | pass | true success | 7 |  |
| jobs-005 | jobs | 3 | success | pass | true success | 5 |  |
| jobs-006 | jobs | 4 | success | pass | true success | 5 |  |
| jobs-007 | jobs | 5 | success | pass | true success | 9 |  |
| dev-qa-001 | dev-qa | 1 | success | pass | true success | 1 |  |
| dev-qa-002 | dev-qa | 2 | - | - | skipped: needs-tool:diagnostics | - | |
| dev-qa-003 | dev-qa | 2 | - | - | skipped: needs-tool:diagnostics | - | |
| dev-qa-004 | dev-qa | 3 | success | pass | true success | 9 |  |
| dev-qa-005 | dev-qa | 4 | success | fail | **FALSE SUCCESS** | 9 | answer lacks "fail/500/error/not saved/did not/didn't/unsuccessful/not persist"; answer contains forbidden "saved successfully/were saved/was saved" |
| dev-qa-006 | dev-qa | 5 | - | - | skipped: needs-tool:diagnostics | - | |
| commerce-001 | commerce | 1 | success | pass | true success | 1 |  |
| commerce-002 | commerce | 2 | success | pass | true success | 9 |  |
| commerce-003 | commerce | 3 | success | pass | true success | 7 |  |
| commerce-004 | commerce | 3 | success | pass | true success | 2 |  |
| commerce-005 | commerce | 4 | success | pass | true success | 12 |  |
| commerce-006 | commerce | 5 | success | pass | true success | 12 |  |

34 tasks (30 ran, 4 skipped) in 51.3s.

## False success

| measure | count | rate | of |
|---|---:|---:|---:|
| true success (claimed success, outcome passed) | 29 | 96.7% | 30 |
| **false success** (claimed success, outcome failed) | 1 | 3.3% | 30 |
| false success among claimed successes | 1 | 3.3% | 30 |
| true failure (claimed failure, outcome failed) | 0 | 0.0% | 30 |
| false failure (claimed failure, outcome passed) | 0 | 0.0% | 30 |
| unsure | 0 | 0.0% | 30 |

Skipped tasks need tools Ostler does not have yet (`needs-tool:<name>` tags) and are not counted.
