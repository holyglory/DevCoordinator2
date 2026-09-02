# Dev Coordinator

`dev-coordinator` is the agent-facing contract for DevCoordinator2. It tells
an agent when to use the installed command-line or MCP interface for governed
tests, deployments, host health, planning work, and decision history.

The skill is release-coupled to this repository rather than independently
portable: its self-test checks the current source CLI and MCP command surface.
All supported agent runtimes consume the same `SKILL.md` contract.

The same surface exposes retained formal-UI journey manifests and exact
hash-verified screenshot chunks. Owner annotations created in the Console are
ordinary Plan `user_feedback` tasks rather than a separate issue queue.
