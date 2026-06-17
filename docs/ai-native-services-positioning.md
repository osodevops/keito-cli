# Keito Positioning: AI-Native Services

Keito should be positioned as billing and profitability infrastructure for
AI-native services companies.

## Why Now

YC's Summer 2026 Request for Startups explicitly calls for **AI-Native Service
Companies**: companies that do not sell software, but sell the completed
service. YC also calls for **Software for Agents**, arguing that agents need
machine-readable APIs, MCPs, CLIs, and documentation so they can use tools
programmatically without a human in the loop.

Sources:
- https://www.ycombinator.com/rfs
- https://github.com/garrytan/gstack

The category implication is direct: if agents are doing client work, the
software around services businesses has to become agent-native too. Billing,
time tracking, project mapping, approvals, and profitability reporting cannot
remain browser-only workflows for humans.

## Category Frame

Keito is not LLM cost observability.

LLM observability answers:
- Which model ran?
- How many tokens did it use?
- What did the inference cost?
- Was latency or quality acceptable?

Keito answers:
- Which client and project did the agent work for?
- What task was delivered?
- What time or effort should flow into billing?
- What did that work cost internally?
- What margin did the client, project, workflow, or agent generate?

Lightspeed's investment note on Paid frames the same gap as missing economic
infrastructure for AI agents: traditional SaaS pricing breaks when agents are
autonomous digital workers, and the missing capabilities are value proof,
custom pricing, outcome and hybrid models, cost tracking, and customer
profitability dashboards.

Source:
- https://lsvp.com/stories/the-ai-agent-economy-has-a-19-trillion-problem-our-investment-in-paid/

## Buyer

Primary buyer:
- AI-native agencies and consultancies
- Professional-services teams using coding, research, support, finance, or
  operations agents
- YC-style startups replacing outsourced services with agent-run delivery

The buyer wants software-margin economics without losing service-business
controls: client attribution, auditability, utilization, approval, invoices,
and profitability.

## Messaging

Use this compact positioning:

> Keito is the billing and profitability layer for AI-native services teams.
> Agents log work against clients, projects, and tasks the same way humans do,
> so firms can invoice accurately, prove value, and see margin by customer and
> workflow.

Supporting claims:
- "Bring Keito to where agents work": ship a CLI plus Codex and Claude Code
  skills, not a dashboard-only workflow.
- "Track billable work, not just token spend": connect agent activity to
  projects, invoices, and margin.
- "One worktree, one client/project/task mapping": repo-local setup keeps
  agent time from leaking across clients.
- "Best-effort hooks, deterministic CLI": agent sessions should never fail
  because billing telemetry failed, but failures must be logged and recoverable.

## Product Implications For The CLI

The CLI should prioritize:
- Bundled agent skills for Codex and Claude Code.
- A gstack-style install flow: install globally, then bootstrap each repo.
- Deterministic `--json` commands and stable exit codes.
- `source=agent` entries with metadata for session ID, agent type, skill, git
  branch, git revision, duration, and draft status.
- Repo-local `.keito/config.yml` selected by a wizard and excluded from git.
- `keito skill status` and `keito skill doctor` for agent-readable readiness.

MindStudio's monetization guide makes the margin reason explicit: successful
AI-agent businesses need disciplined cost tracking and 60-75% gross margins;
cost creep at the interaction level can erase margin. Keito should connect
that cost discipline to the professional-services billing system.

Source:
- https://www.mindstudio.ai/blog/build-monetize-ai-agents-business
