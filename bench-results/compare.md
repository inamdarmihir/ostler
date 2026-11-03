| task | server | steps | out tokens (est) | args tokens (est) | wall ms | correct (server-side) |
|---|---|---:|---:|---:|---:|---:|
| basic | Ostler | 3 | 227 | 46 | 526 | 3/3 |
| basic | Ostler (+form_inspect) | 4 | 220 | 45 | 482 | 3/3 |
| basic | Playwright MCP | 4 | 355 | 95 | 710 | 3/3 |
| basic | Chrome DevTools MCP | 4 | 168 | 78 | 840 | 3/3 |
| validation | Ostler | 3 | 202 | 36 | 497 | 3/3 |
| validation | Ostler (+form_inspect) | 4 | 190 | 34 | 488 | 3/3 |
| validation | Playwright MCP | 4 | 300 | 72 | 1,116 | 3/3 |
| validation | Chrome DevTools MCP | 4 | 156 | 71 | 1,287 | 3/3 |
| dependent | Ostler | 3 | 238 | 42 | 844 | 3/3 |
| dependent | Ostler (+form_inspect) | 4 | 210 | 40 | 866 | 3/3 |
| dependent | Playwright MCP | 5 | 389 | 94 | 1,921 | 3/3 |
| dependent | Chrome DevTools MCP | 6 | 476 | 98 | 1,390 | 3/3 |
| wizard | Ostler | 7 | 396 | 60 | 1,036 | 3/3 |
| wizard | Ostler (+form_inspect) | 8 | 375 | 59 | 1,022 | 3/3 |
| wizard | Playwright MCP | 10 | 784 | 134 | 2,740 | 3/3 |
| wizard | Chrome DevTools MCP | 10 | 372 | 142 | 2,008 | 3/3 |
| longform | Ostler | 3 | 487 | 173 | 1,174 | 3/3 |
| longform | Ostler (+form_inspect) | 4 | 501 | 172 | 1,084 | 3/3 |
| longform | Playwright MCP | 4 | 1,132 | 434 | 2,268 | 3/3 |
| longform | Chrome DevTools MCP | 4 | 694 | 240 | 2,394 | 3/3 |

| server | version | tools | tool schema (bytes) | schema tokens (est) | cold start ms |
|---|---|---:|---:|---:|---:|
| Ostler | 0.2.0 (npm ostler 0.2.0) | 21 | 6,051 | 1,513 | 693 |
| Ostler (+form_inspect) | 0.2.0 (npm ostler 0.2.0) | 21 | 6,051 | 1,513 | 1,332 |
| Playwright MCP | 1.65.0-alpha-2026-10-30 (npm 0.0.84) | 25 | 21,060 | 5,265 | 1,864 |
| Chrome DevTools MCP | 1.12.0 (npm 1.12.0) | 32 | 29,661 | 7,416 | 7,653 |

| server | steps (sum of 5 tasks) | out tokens (sum) | wall ms (sum) | tasks correct |
|---|---:|---:|---:|---:|
| Ostler | 19 | 1,550 | 4,077 | 5/5 |
| Ostler (+form_inspect) | 24 | 1,496 | 3,942 | 5/5 |
| Playwright MCP | 27 | 2,960 | 8,755 | 5/5 |
| Chrome DevTools MCP | 28 | 1,866 | 7,919 | 5/5 |

Sequences:
- Ostler: navigate(interactive) -> form_fill -> form_submit (per wizard step)
- Ostler (+form_inspect): navigate(none) -> form_inspect -> form_fill -> form_submit
- Playwright MCP: browser_navigate -> browser_snapshot -> browser_fill_form -> browser_click
- Chrome DevTools MCP: navigate_page -> take_snapshot -> fill_form -> click
