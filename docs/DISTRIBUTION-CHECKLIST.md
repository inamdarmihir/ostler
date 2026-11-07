# Distribution checklist

Places where people find MCP servers, so Ostler can be installed without a search. Status is what is true in this repository; ticking a box needs a person with the account. Back to the [accessibility plan](ACCESSIBILITY-PLAN.md).

| where | what it needs | status |
|---|---|---|
| GitHub Releases | push a `v*` tag ([RELEASING](RELEASING.md)); builds binaries and the macOS `.mcpb` | not done |
| PyPI (`ostler-mcp`) | `PYPI_TOKEN` secret ([RELEASING](RELEASING.md)); wheel job exists | blocked on first release |
| VS Code Marketplace and Open VSX | `VSCE_PAT` and `OVSX_PAT` secrets, claim the publisher and namespace | blocked on first release |
| Official MCP registry | `server.json` exists; publish after the npm package is live | blocked on first release |
| Glama | `glama.json` exists; claim the listing | to do |
| Smithery, mcp.so, PulseMCP | submit the repo; give the `npx -y ostler serve` command | to do |
| Cursor and VS Code directories | install links are in [GETTING-STARTED](GETTING-STARTED.md); submit for listing | to do |
| Claude Desktop extension directory | submit the `.mcpb` after it is verified on a Mac | blocked on first release and Mac check |
| Homebrew tap | formula generator exists; set the `HOMEBREW_TAP_TOKEN` secret | to do |
| awesome-mcp-servers lists | open a pull request with a one-line entry | to do |

Not built: an update notice. It needs a network call to check for new versions, which Ostler does not make today; add it only as an explicit opt-in.
