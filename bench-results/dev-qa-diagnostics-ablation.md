# `diagnostics` ablation on the dev-qa pack (scripted oracle, no model)

Same script, same fixtures, same scorer. The only difference is whether the script calls `diagnostics` after
clicking Save, or trusts the page's own "Settings saved" toast.

```sh
OSTLER_SCRIPTED_CHECKS=0 ostler-bench packs --scripted --pack dev-qa   # trusts the toast
OSTLER_SCRIPTED_CHECKS=1 ostler-bench packs --scripted --pack dev-qa   # checks diagnostics
```

| arm | tasks | true success | false success | dev-qa-004 (save works) | dev-qa-005 (save fails with 500) |
|---|---:|---:|---:|---|---|
| trusts the toast | 6 | 5 | **1 (16.7%)** | true success, 9 calls | **false success**: "settings saved successfully" |
| checks `diagnostics` | 6 | 6 | **0** | true success, 10 calls | true success: reports the failed request |

Read this carefully:

- The script is an oracle that knows the page layout. It shows that **the signal needed to catch the false
  success is in `diagnostics`, and costs one extra call**; it does not show that a model will call the tool.
  That needs the model-in-the-loop run (`scripts/eval/`), with and without the tool listed.
- `dev-qa-004` is the control: a correct save is still reported as a success.
- One planted-trap task is one task. This is a demonstration of mechanism, not a rate.
- `diagnostics` also reports ordinary noise (a missing favicon is a 404). The scripted agent ignores it; a model
  has to.
