# Four agents: separate Ostler processes vs one shared Ostler (measured)

Each agent reads one fixture page. Browser process-tree memory after the reads.

| setup | browsers | memory (MiB) |
|---|---:|---:|
| 4 Ostler processes, one Chrome each | 4 | 813 |
| 1 Ostler, 4 tabs (`--swarm`, explicit `tab`) | 1 | 388 |

One machine (Linux container), headless Chrome, small static pages. Real pages are heavier and share less. The cost of sharing is that agents must pass `tab` on every page call, and that calls on one tab take turns.
