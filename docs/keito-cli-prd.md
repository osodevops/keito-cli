# Keito CLI — Product Requirements Document

**Product**: keito-cli  
**Version**: 1.0.0  
**Author**: Keito Engineering  
**Date**: 2026-03-04  
**Status**: Draft  

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Problem Statement](#2-problem-statement)
3. [Market Research & Competitive Landscape](#3-market-research--competitive-landscape)
4. [Community Insights (Reddit & Industry)](#4-community-insights-reddit--industry)
5. [Target Users](#5-target-users)
6. [Product Vision & Design Principles](#6-product-vision--design-principles)
7. [Architecture Overview](#7-architecture-overview)
8. [CLI Command Reference](#8-cli-command-reference)
9. [OpenClaw Skill Integration](#9-openclaw-skill-integration)
10. [Authentication & Configuration](#10-authentication--configuration)
11. [Data Model](#11-data-model)
12. [Agent-Specific Features](#12-agent-specific-features)
13. [Documentation Strategy](#13-documentation-strategy)
14. [Error Handling & Resilience](#14-error-handling--resilience)
15. [Testing Strategy](#15-testing-strategy)
16. [Release Plan & Distribution](#16-release-plan--distribution)
17. [Success Metrics](#17-success-metrics)
18. [Future Roadmap](#18-future-roadmap)
19. [Appendix A: Full Command Tree](#appendix-a-full-command-tree)
20. [Appendix B: OpenClaw SKILL.md Template](#appendix-b-openclaw-skillmd-template)
21. [Appendix C: Example Agent Workflows](#appendix-c-example-agent-workflows)

---

## 1. Executive Summary

**keito-cli** is a command-line tool that enables both humans and AI agents (OpenClaw, Claude Code, Codex, Cursor agents) to record billable time against projects in Keito — a time tracking and invoicing platform for professional services.

The core insight: as AI agents increasingly perform real work (code generation, research, document drafting, data analysis), professional services firms need a way to track and bill for agent work hours. No product on the market currently solves this.

keito-cli is designed with a **CLI-first, agent-native** philosophy — every command is deterministic, requires no interactive prompts, returns structured output, and can be executed by an autonomous agent in a terminal session.

Alongside the CLI binary, we ship an **OpenClaw skill** (`keito-time`) that teaches OpenClaw agents how to discover projects, record time, and manage entries using the CLI — without injecting full API documentation into the agent's context window.

---

## 2. Problem Statement

### The Gap

Professional services firms are deploying AI agents to perform billable work — writing code, generating reports, conducting research, reviewing contracts. But there is currently **no mechanism for agents to self-report their time** against client projects in a way that flows into existing billing and invoicing workflows.

### Current Pain Points

1. **Lost revenue**: Agent work goes untracked because no human remembers to log it. Professional services firms already lose 15-25% of billable hours to poor tracking (TimeRewards, 2025). Agent hours compound this.
2. **No billing unit for agents**: As highlighted in r/fintech discussions, "autonomous agents don't have a natural billing unit" — time-based tracking provides a familiar, auditable model.
3. **Context overhead**: Giving an agent raw API docs consumes significant context window. Agents need a direct CLI with `--help` for self-discovery, not 50-page API references.
4. **No audit trail**: When agents perform work, there's no structured record of what was done, for which client, and for how long.
5. **Billing disputes**: Without contemporaneous time tracking by agents, firms cannot justify charges to clients.

### What We're Building

A CLI binary + OpenClaw skill that:
- Lets agents start/stop timers and log duration entries against Keito projects
- Produces structured JSON output for programmatic consumption
- Includes self-documenting `--help` at every level
- Ships with an OpenClaw skill so agents can discover and use it without explicit instruction
- Tags entries as agent-created for audit and reporting purposes

---

## 3. Market Research & Competitive Landscape

### Direct Competitors (Agent Time Tracking for Billing)

**None exist.** No product currently enables AI agents to record billable hours against client projects in a professional services context. This is a greenfield opportunity.

### Adjacent Products

| Product | What It Does | Gap vs. keito-cli |
|---|---|---|
| **AgentBudget** | Real-time LLM cost enforcement per session | Tracks API spend, not billable client hours |
| **Paid.ai** | AI agent cost tracking + billing automation | Focuses on SaaS margin optimization, not professional services time tracking |
| **AgentPaid** | AI agent billing observability | Cost monitoring, not time-entry creation |
| **GitScrum MCP** | MCP server with 9 time-tracking actions | Tied to GitScrum ecosystem, no standalone CLI |
| **Harvest MCP Server** | MCP server for Harvest time tracking | Requires MCP-compatible client, no raw CLI |
| **Zapier MCP + Time Tracker** | Connect time tracking via MCP/Zapier | No-code glue, not a native CLI tool |
| **HCl / hrvst-cli** | CLI tools for Harvest | Designed for humans, no agent-native features (no JSON output, no agent tagging) |
| **Billables.ai** | Automated legal timekeeping | Monitors human desktop activity, not agent terminal sessions |
| **Laurel AI** | AI-powered timesheet generation | Enterprise legal/accounting focus, passive capture only |
| **ChronoAI** | Natural language time input + invoicing | Consumer-grade, no API/CLI, not agent-aware |

### Key Takeaway

The market has two clusters: (1) tools that track **AI costs** (tokens, API calls) and (2) tools that help **humans** track time with AI assistance. Nothing sits in the middle — enabling **agents to track their own billable time** against client projects. keito-cli occupies this whitespace.

---

## 4. Community Insights (Reddit & Industry)

Research across Reddit communities and industry publications surfaced several themes that directly informed this PRD:

### Theme 1: Agents Need a Billing Model
> *"Autonomous agents don't have a natural billing unit. That's the real [problem]."* — r/fintech (March 2026)

Time-based billing provides a familiar, auditable unit that clients already understand. Rather than inventing a new billing primitive, keito-cli maps agent work to the same time-entry model used by human team members.

**Design impact**: Agent time entries use the same data model as human entries (date, project, task, duration, notes, billable status) but include metadata identifying the source as an agent.

### Theme 2: Freelancers Already Question Agent Billing
> *"Should Freelancers Log Time When AI Agents Are Writing Code? When these tools handle most of the coding, my keyboard and mouse usage drops significantly while I supervise..."* — r/Upwork

This shows billing for agent work is already a live debate. Firms need a tool that provides transparent records showing when an agent worked and what it produced.

**Design impact**: Every time entry includes a `source` field (`agent` vs `human`) and structured `notes` describing the work performed, so invoices can clearly attribute agent vs. human effort.

### Theme 3: CLI Over API for Agents
> *"A more efficient approach is to provide agents with a direct CLI... they can simply execute commands in their terminal. For instance, they can type `moltbook --help` to view all available commands."* — r/SideProject

Agents consume CLIs far more efficiently than raw API documentation. A CLI with comprehensive `--help` is self-documenting and doesn't require embedding API specs into the agent's context.

**Design impact**: Every command and subcommand has detailed `--help`. Output defaults to human-readable tables but supports `--json` for programmatic use. No interactive prompts.

### Theme 4: The End of Billable Hours Is Exaggerated
> *"AI-first approach: 5-person squad with AI agents, 8–12 week timeline, $800K. Same outcome. 84% cost reduction."* — LinkedIn (February 2026)

Professional services firms won't stop billing for time — they'll bill for agent time at different rates. Firms need infrastructure to track both.

**Design impact**: Support for per-agent billing rates and clear separation of agent vs. human entries in reports and invoices.

### Theme 5: Cost Tracking Needs Are Growing
> *"I built an open-source billing engine for AI Agents — track costs per customer/agent in real-time"* — r/AI_Agents

Developers are building ad-hoc solutions for agent cost tracking. There's demand for a structured, product-grade approach.

**Design impact**: The CLI tracks both time (for client billing) and optionally surfaces estimated LLM costs (for internal margin analysis) via metadata fields.

---

## 5. Target Users

### Primary: AI Agents (OpenClaw, Claude Code, Codex, Cursor CLI)
- Execute `keito` commands in terminal sessions
- Consume `--help` for self-discovery
- Parse `--json` output for decision-making
- Operate autonomously without human prompting

### Secondary: Developers & DevOps Engineers
- Configure keito-cli for agent environments
- Set up authentication and workspace defaults
- Build automation pipelines that include time tracking

### Tertiary: Professional Services Managers
- Review agent-created time entries in Keito web UI
- Approve/reject agent timesheets
- Analyze agent utilization and billable hours

---

## 6. Product Vision & Design Principles

### Vision
Every AI agent that performs billable work should track its time like any other team member. keito-cli makes this as natural as `git commit`.

### Design Principles

1. **Agent-native by default**: No interactive prompts. No browser-based auth flows during use. All commands accept all inputs as flags/arguments. Output is structured and parseable.

2. **Self-documenting**: Comprehensive `--help` at every level. Agents should be able to run `keito --help` and `keito time --help` to discover capabilities without external docs.

3. **Deterministic**: Same input → same output. No fuzzy matching, no "did you mean?" suggestions. Commands either succeed or fail with clear exit codes and error messages.

4. **Human-compatible**: While designed for agents, every command works perfectly for humans too. Human-readable table output by default, `--json` for agents.

5. **Minimal context footprint**: The OpenClaw skill teaches usage patterns without embedding full API docs. The CLI is the interface, not the API.

6. **Auditable**: Every entry created by the CLI is tagged with source metadata (agent ID, session ID, tool chain) for compliance and billing transparency.

7. **Offline-resilient**: Commands that can be queued locally (time entries) should support offline buffering with sync on reconnection.

---

## 7. Architecture Overview

```
┌─────────────────────────────────────────────────────┐
│                   Agent Runtime                      │
│  (OpenClaw / Claude Code / Codex / Cursor)          │
│                                                      │
│  ┌──────────────┐    ┌──────────────────────┐       │
│  │ OpenClaw     │    │ Agent executes:       │       │
│  │ Skill:       │───▶│ $ keito time start    │       │
│  │ keito-time   │    │   --project acme-web  │       │
│  └──────────────┘    │   --task development  │       │
│                      └──────────┬───────────┘       │
└─────────────────────────────────┼───────────────────┘
                                  │
                    ┌─────────────▼─────────────┐
                    │       keito-cli binary     │
                    │                            │
                    │  • Auth (API key / token)  │
                    │  • Command parsing         │
                    │  • Input validation        │
                    │  • Offline queue           │
                    │  • JSON/table output       │
                    └─────────────┬─────────────┘
                                  │ HTTPS
                    ┌─────────────▼─────────────┐
                    │     Keito REST API         │
                    │                            │
                    │  /api/v1/time-entries      │
                    │  /api/v1/projects          │
                    │  /api/v1/tasks             │
                    │  /api/v1/clients           │
                    │  /api/v1/timers            │
                    └─────────────┬─────────────┘
                                  │
                    ┌─────────────▼─────────────┐
                    │     Keito Platform         │
                    │                            │
                    │  Web UI / Reports /        │
                    │  Invoicing / Xero sync     │
                    └───────────────────────────┘
```

### Technology Choices

| Component | Choice | Rationale |
|---|---|---|
| Language | **Rust** | Single binary, fast startup, cross-platform, no runtime deps. Aligns with team expertise. |
| CLI Framework | **clap** (Rust) | Derive macros for auto-generated help, completions, structured parsing |
| HTTP Client | **reqwest** | Async, TLS built-in, well-maintained |
| Config | **TOML** (~/.config/keito/config.toml) | Human-readable, Rust-native serde support |
| Output | **tabled** (tables) + **serde_json** (JSON) | Dual output modes |
| Auth storage | **keyring** crate (OS keychain) | Secure credential storage, fallback to env var |
| Distribution | **Homebrew**, **cargo install**, GitHub releases (Linux/macOS/Windows binaries) | Covers all agent runtime environments |

---

## 8. CLI Command Reference

### Global Flags

```
keito [OPTIONS] <COMMAND>

Options:
  --json              Output as JSON (default for piped stdout)
  --workspace <ID>    Override workspace (default: from config)
  --profile <NAME>    Use named auth profile
  --quiet             Suppress non-essential output
  --verbose           Enable debug logging
  --version           Print version
  --help              Print help
```

### Commands Overview

```
keito
├── auth
│   ├── login         # Store API key (interactive, one-time setup)
│   ├── logout        # Remove stored credentials
│   ├── status        # Check authentication status
│   └── whoami        # Show current user/agent identity
├── time
│   ├── start         # Start a timer
│   ├── stop          # Stop the running timer
│   ├── log           # Log a completed time entry (duration-based)
│   ├── list          # List time entries (with filters)
│   ├── edit          # Edit an existing entry
│   ├── delete        # Delete an entry
│   └── running       # Show currently running timer
├── projects
│   ├── list          # List available projects
│   ├── show          # Show project details (tasks, budget, team)
│   └── tasks         # List tasks for a project
├── clients
│   ├── list          # List clients
│   └── show          # Show client details and projects
├── reports
│   ├── summary       # Time summary (by project, task, date range)
│   └── entries       # Detailed entry export
├── config
│   ├── show          # Show current configuration
│   ├── set           # Set a config value
│   └── init          # Initialize config file interactively
├── sync              # Flush offline queue to server
└── completions       # Generate shell completions (bash/zsh/fish)
```

### Detailed Command Specifications

#### `keito time start`

Start a timer for a project and task.

```
keito time start [OPTIONS]

Options:
  --project <SLUG>       Project slug or ID (required)
  --task <SLUG>          Task slug or ID (required)
  --notes <TEXT>         Description of work being performed
  --billable <BOOL>      Override billable status (default: project setting)
  --source <SOURCE>      Entry source identifier (default: "cli")
  --agent-id <ID>        Agent identifier for tracking (auto-detected from env)
  --session-id <ID>      Session identifier for grouping entries
  --metadata <JSON>      Arbitrary JSON metadata (e.g., LLM model, token count)
```

**Example (agent use)**:
```bash
keito time start \
  --project acme-website \
  --task development \
  --notes "Implementing user authentication module" \
  --agent-id openclaw-ops-01 \
  --session-id sess_abc123
```

**Output (JSON)**:
```json
{
  "status": "started",
  "timer_id": "tmr_7f3a2b",
  "project": "acme-website",
  "task": "development",
  "started_at": "2026-03-04T15:30:00Z"
}
```

**Exit codes**: 0 = success, 1 = auth error, 2 = invalid project/task, 3 = timer already running

---

#### `keito time stop`

Stop the currently running timer.

```
keito time stop [OPTIONS]

Options:
  --notes <TEXT>         Append to or replace notes
  --discard              Stop without saving the entry
  --round <INCREMENT>    Round duration (5m, 6m, 10m, 15m, 30m)
```

**Example**:
```bash
keito time stop --notes "Completed auth module, passing all tests"
```

**Output (JSON)**:
```json
{
  "status": "stopped",
  "entry_id": "ent_9c4d1e",
  "project": "acme-website",
  "task": "development",
  "duration": "1:45",
  "duration_hours": 1.75,
  "billable": true,
  "started_at": "2026-03-04T15:30:00Z",
  "stopped_at": "2026-03-04T17:15:00Z"
}
```

---

#### `keito time log`

Log a completed time entry without using a timer.

```
keito time log [OPTIONS]

Options:
  --project <SLUG>       Project slug or ID (required)
  --task <SLUG>          Task slug or ID (required)
  --duration <DURATION>  Duration in decimal hours or HH:MM (required)
  --date <DATE>          Date of work (default: today, format: YYYY-MM-DD)
  --notes <TEXT>         Description of work performed
  --billable <BOOL>      Override billable status
  --start-time <TIME>    Start time (HH:MM, optional)
  --end-time <TIME>      End time (HH:MM, optional, alternative to --duration)
  --source <SOURCE>      Entry source identifier
  --agent-id <ID>        Agent identifier
  --session-id <ID>      Session identifier
  --metadata <JSON>      Arbitrary JSON metadata
```

**Example (agent logging completed work)**:
```bash
keito time log \
  --project acme-website \
  --task code-review \
  --duration 0.5 \
  --notes "Reviewed PR #142: Added input validation to API endpoints" \
  --agent-id openclaw-ops-01
```

---

#### `keito time list`

List time entries with filters.

```
keito time list [OPTIONS]

Options:
  --from <DATE>          Start date (YYYY-MM-DD, default: start of week)
  --to <DATE>            End date (YYYY-MM-DD, default: today)
  --project <SLUG>       Filter by project
  --task <SLUG>          Filter by task
  --billable <BOOL>      Filter by billable status
  --source <SOURCE>      Filter by source (e.g., "agent", "cli", "web")
  --agent-id <ID>        Filter by agent ID
  --limit <N>            Max entries to return (default: 50)
  --page <N>             Page number for pagination
```

---

#### `keito projects list`

List projects available to the authenticated user/agent.

```
keito projects list [OPTIONS]

Options:
  --client <SLUG>        Filter by client
  --active               Only active projects (default: true)
  --with-tasks           Include task list per project
  --limit <N>            Max results
```

**Output (JSON)**:
```json
[
  {
    "id": "proj_abc123",
    "slug": "acme-website",
    "name": "Acme Corp Website Redesign",
    "client": "Acme Corp",
    "active": true,
    "billable": true,
    "budget_hours": 200,
    "used_hours": 87.5,
    "tasks": ["development", "design", "meetings", "code-review"]
  }
]
```

---

#### `keito projects tasks`

List tasks for a specific project.

```
keito projects tasks <PROJECT_SLUG>
```

**Output (JSON)**:
```json
[
  { "id": "tsk_001", "slug": "development", "name": "Development", "billable": true },
  { "id": "tsk_002", "slug": "design", "name": "Design", "billable": true },
  { "id": "tsk_003", "slug": "meetings", "name": "Meetings", "billable": true },
  { "id": "tsk_004", "slug": "internal", "name": "Internal", "billable": false }
]
```

---

#### `keito reports summary`

Generate a time summary report.

```
keito reports summary [OPTIONS]

Options:
  --from <DATE>          Start date (required)
  --to <DATE>            End date (required)
  --group-by <FIELD>     Group by: project, task, date, agent-id (default: project)
  --project <SLUG>       Filter by project
  --agent-id <ID>        Filter by agent
  --billable-only        Include only billable entries
```

---

## 9. OpenClaw Skill Integration

### Skill: `keito-time`

The OpenClaw skill teaches agents how to use keito-cli for time tracking. It follows the AgentSkills-compatible format.

#### File Structure

```
skills/keito-time/
├── SKILL.md                     # Core workflow (under 500 lines)
├── references/
│   ├── command-reference.md     # Full CLI command docs
│   ├── common-workflows.md      # Step-by-step agent workflows
│   └── error-handling.md        # Error codes and recovery
└── scripts/
    └── setup-check.sh           # Verify keito-cli installation
```

#### SKILL.md Design

The SKILL.md file follows the principle of teaching the agent **when** and **how** to use keito-cli, without embedding the full API surface. The agent discovers specific commands via `--help`.

**Key sections in SKILL.md**:

1. **Trigger conditions**: When should the agent use this skill? (Starting work on a project, completing a task, switching tasks)
2. **Discovery workflow**: Run `keito projects list --json` → pick project → run `keito projects tasks <slug> --json` → pick task → start timer
3. **Core patterns**: Start timer → do work → stop timer (with notes summarizing what was done)
4. **Duration logging**: For work already completed, use `keito time log` with calculated duration
5. **Error recovery**: If timer already running, stop it first. If project not found, list projects and retry.
6. **Guardrails**: Never log more than 8 hours per entry. Always include descriptive notes. Always tag with agent-id.

#### How OpenClaw Loads the Skill

Per OpenClaw's skill system:

1. **Installation**: `clawhub install keito-time` (or copy to `~/.openclaw/skills/keito-time/`)
2. **Gating**: The skill declares `metadata.openclaw.requires.bins: ["keito"]` — it only activates when the `keito` binary is on PATH
3. **Environment**: The skill declares `metadata.openclaw.primaryEnv: "KEITO_API_KEY"` for auth
4. **Auto-discovery**: On each session start, OpenClaw detects the skill and includes it in the agent's context
5. **References loaded on-demand**: The `references/` folder content is only pulled when the agent invokes the skill

#### Token Budget

Per OpenClaw's token impact formula:
- Base overhead: 195 characters (when ≥1 skill)
- Per-skill: ~97 + name + description + location ≈ ~250 characters for keito-time
- Estimated: ~62 tokens added to system prompt (minimal impact)

Full command reference lives in `references/` and is loaded only when the skill activates.

---

## 10. Authentication & Configuration

### Auth Flow

```
# One-time setup (human-driven):
keito auth login

# Prompts for:
# 1. Keito API key (from keito.ai/settings/api)
# 2. Workspace ID (auto-detected if only one)
# Stores in OS keychain via `keyring` crate

# For agent/CI environments:
export KEITO_API_KEY=keit_xxxxxxxxxxxxx
export KEITO_WORKSPACE_ID=ws_abc123

# Verify:
keito auth whoami --json
# {"user_id": "usr_123", "name": "Agent: OpenClaw Ops", "workspace": "Acme Consulting", "role": "member"}
```

### Configuration File

Location: `~/.config/keito/config.toml`

```toml
[default]
workspace_id = "ws_abc123"
default_output = "table"      # "table" | "json"
timezone = "Europe/London"

[agent]
default_agent_id = "openclaw-ops-01"   # Auto-populate --agent-id
default_source = "agent"                # Tag entries as agent-created
auto_notes = true                       # Require notes on every entry

[offline]
enabled = true
queue_path = "~/.local/share/keito/queue.jsonl"
max_queue_size = 100

[rounding]
default = "6m"               # Round to nearest 6 minutes (1/10 hour)
```

### Auth Precedence

1. `--workspace` flag (highest)
2. `KEITO_WORKSPACE_ID` env var
3. Config file `workspace_id`
4. API key from env: `KEITO_API_KEY`
5. API key from OS keychain (lowest, human setup)

---

## 11. Data Model

### Time Entry (as created by CLI)

```json
{
  "id": "ent_9c4d1e",
  "date": "2026-03-04",
  "project_id": "proj_abc123",
  "project_slug": "acme-website",
  "task_id": "tsk_001",
  "task_slug": "development",
  "duration_hours": 1.75,
  "notes": "Implemented user authentication module with JWT tokens",
  "billable": true,
  "started_at": "2026-03-04T15:30:00Z",
  "stopped_at": "2026-03-04T17:15:00Z",
  "source": "cli",
  "agent_metadata": {
    "agent_id": "openclaw-ops-01",
    "agent_type": "openclaw",
    "session_id": "sess_abc123",
    "model": "claude-4-sonnet",
    "custom": {}
  },
  "created_at": "2026-03-04T17:15:00Z",
  "locked": false,
  "approval_status": "pending"
}
```

### Key Design Decisions

1. **`source` field**: Distinguishes CLI entries from web UI entries. Values: `cli`, `web`, `api`, `agent`.
2. **`agent_metadata` object**: Optional nested object for agent-specific data. Only present when `--agent-id` is provided.
3. **Slug-based references**: Projects and tasks can be referenced by slug (human-readable) or ID. Slugs are auto-generated from names, lowercase, hyphenated.
4. **Duration as decimal hours**: Internal representation is decimal hours (1.75 = 1h45m). Display supports both decimal and HH:MM.

---

## 12. Agent-Specific Features

### 12.1 Automatic Agent Detection

The CLI auto-detects agent context from environment variables:

| Env Var | Detected Agent Type |
|---|---|
| `OPENCLAW_AGENT_ID` | OpenClaw |
| `CLAUDE_SESSION_ID` | Claude Code |
| `CODEX_SESSION_ID` | OpenAI Codex |
| `CURSOR_AGENT` | Cursor |

When detected, the CLI auto-populates `--agent-id` and `--source agent` without explicit flags.

### 12.2 Session Tracking

Agents can pass `--session-id` to group related time entries within a single work session. This enables:
- Viewing all entries from one agent session
- Calculating total session cost/time
- Correlating time entries with agent logs

### 12.3 Structured Notes

Agents are encouraged (and the OpenClaw skill enforces) structured notes:

```
keito time stop --notes "Completed: Implemented JWT auth. Files: src/auth.rs, src/middleware.rs. Tests: 12 passing. PR: #142"
```

The notes field is free-text but the skill teaches agents to use a consistent format:
`Completed: <summary>. Files: <files>. Tests: <test status>. PR: <link>.`

### 12.4 Metadata Pass-Through

The `--metadata` flag accepts arbitrary JSON for internal tracking:

```bash
keito time log \
  --project acme-website \
  --task development \
  --duration 1.5 \
  --notes "Generated API documentation" \
  --metadata '{"model":"claude-4-sonnet","input_tokens":15000,"output_tokens":8000,"estimated_cost_usd":0.12}'
```

This data is stored but not displayed on invoices — it's for internal cost analysis.

### 12.5 Offline Queue

When the Keito API is unreachable, entries are queued locally:

```bash
# Entries queued automatically when offline
keito time log --project acme-website --task dev --duration 1.0
# Warning: Offline. Entry queued (1 pending).

# Manual sync when back online
keito sync
# Synced 3 entries. 0 failures.
```

Queue format: JSONL file at `~/.local/share/keito/queue.jsonl`

---

## 13. Documentation Strategy

### In-CLI Documentation

Every command has:
- `--help` with usage examples
- Error messages that suggest the correct command
- `keito <command> --help` shows all flags with defaults

### Published Documentation (keito.ai/docs/cli)

Pages to create:

1. **Getting Started**: Install, authenticate, first time entry
2. **CLI Reference**: Full command tree with examples
3. **Agent Setup Guide**: Configure keito-cli for OpenClaw/Claude Code/Codex
4. **OpenClaw Skill**: How to install and configure the keito-time skill
5. **Agent Workflows**: Common patterns (start-work-stop, batch logging, session tracking)
6. **Configuration Reference**: All config.toml options
7. **Troubleshooting**: Common errors and fixes
8. **API Key Management**: Creating and rotating keys for agents

### OpenClaw References

Files in `skills/keito-time/references/`:

- `command-reference.md`: Mirror of CLI --help, optimized for agent consumption
- `common-workflows.md`: Step-by-step patterns the agent should follow
- `error-handling.md`: Error codes, what they mean, how to recover
- `project-discovery.md`: How to find the right project and task

---

## 14. Error Handling & Resilience

### Exit Codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Authentication error (missing/invalid API key) |
| 2 | Invalid input (bad project slug, missing required field) |
| 3 | Conflict (timer already running, entry locked) |
| 4 | Not found (project, task, or entry doesn't exist) |
| 5 | Rate limited |
| 6 | Server error (5xx from API) |
| 7 | Network error (offline, timeout) |
| 8 | Configuration error |

### Error Output (JSON mode)

```json
{
  "error": true,
  "code": 2,
  "message": "Project 'acme-websit' not found. Did you mean 'acme-website'?",
  "suggestion": "keito projects list --json",
  "details": {
    "available_projects": ["acme-website", "acme-mobile-app", "internal-tools"]
  }
}
```

**Note**: In `--json` mode, fuzzy suggestions are provided in the response body (not interactively). The agent can parse `details.available_projects` and retry.

### Retry Logic

- Network errors: retry 3x with exponential backoff (1s, 2s, 4s)
- Rate limits: respect `Retry-After` header
- Server errors: retry 2x, then queue offline

---

## 15. Testing Strategy

### Unit Tests
- Command parsing and validation
- Config file loading and precedence
- Duration parsing (decimal, HH:MM, start/end)
- Offline queue serialization

### Integration Tests
- Auth flow (login, whoami, logout)
- CRUD operations against Keito staging API
- Timer lifecycle (start → running → stop)
- Offline queue → sync cycle
- Agent metadata propagation

### Agent Simulation Tests
- Simulate OpenClaw agent executing common workflows
- Verify `--json` output is parseable
- Verify error recovery patterns (wrong project → list → retry)
- Verify skill SKILL.md triggers correct command sequences

### E2E Tests
- Full workflow: auth → list projects → start timer → do work → stop timer → verify entry in Keito web UI
- Cross-platform: macOS, Linux, Windows (GitHub Actions matrix)

---

## 16. Release Plan & Distribution

### Phase 1: Alpha (Week 1-3)
- Core commands: `auth`, `time start/stop/log/list`, `projects list/tasks`
- JSON output
- Config file support
- Homebrew formula (tap)

### Phase 2: Beta (Week 4-6)
- OpenClaw skill (`keito-time`) published to ClawHub
- Offline queue and sync
- Shell completions
- Reports commands
- `cargo install keito-cli`

### Phase 3: GA (Week 7-8)
- GitHub releases with cross-compiled binaries
- Documentation site pages
- Agent auto-detection
- Metadata pass-through
- npm/npx distribution option

### Distribution Channels

| Channel | Command | Target |
|---|---|---|
| Homebrew | `brew install keito-ai/tap/keito` | macOS/Linux developers |
| Cargo | `cargo install keito-cli` | Rust developers |
| npm | `npx keito-cli` | JS/TS environments |
| GitHub Releases | Direct binary download | CI/CD, Docker, agents |
| ClawHub | `clawhub install keito-time` | OpenClaw skill only |

---

## 17. Success Metrics

### Adoption
- Number of CLI installations (Homebrew, cargo, npm, downloads)
- Number of OpenClaw skill installs from ClawHub
- Number of unique agent-ids creating time entries

### Engagement
- Time entries created via CLI per week
- Ratio of agent vs. human CLI entries
- Average session length (start → stop)
- Offline queue usage rate

### Revenue Impact
- Billable hours captured via CLI / agent
- New workspace signups attributed to CLI usage
- Conversion from free → paid driven by agent time tracking

### Quality
- CLI crash rate (target: <0.1%)
- API error rate from CLI (target: <1%)
- Average command latency (target: <500ms)

---

## 18. Future Roadmap

### v1.1 — MCP Server
- Ship a Keito MCP server alongside the CLI
- Enables direct integration with Claude Desktop, Cursor, and other MCP-compatible clients
- Mirrors CLI commands as MCP tools

### v1.2 — Agent Billing Rates
- Configure per-agent hourly rates in Keito
- Agent time entries auto-calculate billable amounts
- Invoice line items show "Agent: OpenClaw Ops — 4.5 hrs @ $75/hr"

### v1.3 — Smart Logging
- CLI can observe git commits and auto-suggest time entries
- `keito time suggest --from-git` analyzes recent commits and proposes entries
- Agent can review suggestions and confirm/edit before logging

### v1.4 — Team Agent Dashboard
- Web UI page showing all agent activity
- Utilization rates per agent
- Cost vs. revenue analysis per agent
- Agent timesheet approval workflow

### v2.0 — Autonomous Billing Pipeline
- Agents create time entries → managers approve → Keito auto-generates invoices → syncs to Xero/QuickBooks
- Zero-touch billing for agent work

---

## Appendix A: Full Command Tree

```
keito --version
keito --help
keito auth login
keito auth logout
keito auth status
keito auth whoami [--json]
keito time start --project <SLUG> --task <SLUG> [--notes <TEXT>] [--billable <BOOL>] [--agent-id <ID>] [--session-id <ID>] [--metadata <JSON>]
keito time stop [--notes <TEXT>] [--discard] [--round <INCREMENT>]
keito time log --project <SLUG> --task <SLUG> --duration <DURATION> [--date <DATE>] [--notes <TEXT>] [--billable <BOOL>] [--start-time <TIME>] [--end-time <TIME>] [--agent-id <ID>] [--session-id <ID>] [--metadata <JSON>]
keito time list [--from <DATE>] [--to <DATE>] [--project <SLUG>] [--task <SLUG>] [--billable <BOOL>] [--source <SOURCE>] [--agent-id <ID>] [--limit <N>] [--page <N>]
keito time edit <ENTRY_ID> [--duration <DURATION>] [--notes <TEXT>] [--billable <BOOL>] [--task <SLUG>]
keito time delete <ENTRY_ID> [--force]
keito time running [--json]
keito projects list [--client <SLUG>] [--active] [--with-tasks] [--limit <N>]
keito projects show <PROJECT_SLUG> [--json]
keito projects tasks <PROJECT_SLUG> [--json]
keito clients list [--limit <N>]
keito clients show <CLIENT_SLUG> [--json]
keito reports summary --from <DATE> --to <DATE> [--group-by <FIELD>] [--project <SLUG>] [--agent-id <ID>] [--billable-only]
keito reports entries --from <DATE> --to <DATE> [--project <SLUG>] [--format csv|json]
keito config show
keito config set <KEY> <VALUE>
keito config init
keito sync [--dry-run]
keito completions <SHELL>
```

---

## Appendix B: OpenClaw SKILL.md Template

```yaml
---
name: keito-time
description: >
  Track billable time against Keito projects. Use when starting work on a client project,
  completing a task, or when asked to log time. Requires the `keito` CLI binary.
metadata: {"openclaw": {"requires": {"bins": ["keito"], "env": ["KEITO_API_KEY"]}, "primaryEnv": "KEITO_API_KEY", "emoji": "⏱️", "homepage": "https://keito.ai/docs/cli"}}
---

# Keito Time Tracking Skill

## When to Use

Activate this skill when:
- You are about to start working on a client project or task
- You have completed a piece of work and need to log time
- The user asks you to track, log, or record time
- You are switching between projects or tasks

## Prerequisites

1. `keito` CLI must be installed and on PATH
2. `KEITO_API_KEY` must be set (or `keito auth login` completed)
3. Verify with: `keito auth whoami --json`

## Core Workflow: Timer-Based

### Step 1: Discover the project
```bash
keito projects list --json
```
Parse the output to find the correct project slug.

### Step 2: Discover the task
```bash
keito projects tasks <project-slug> --json
```
Select the appropriate task for the work you're about to do.

### Step 3: Start the timer
```bash
keito time start --project <slug> --task <slug> --notes "Starting: <brief description>"
```

### Step 4: Do the work
Perform the actual task (write code, generate report, etc.)

### Step 5: Stop the timer
```bash
keito time stop --notes "Completed: <summary of work done>. Files: <files touched>."
```

## Core Workflow: Duration-Based

For work already completed, skip the timer:
```bash
keito time log --project <slug> --task <slug> --duration <hours> --notes "<description>"
```

## Rules

1. **Always include notes** describing what was done
2. **Never log more than 8 hours** in a single entry
3. **Check for running timers** before starting a new one: `keito time running --json`
4. **Use project slugs** from `keito projects list`, never guess
5. If a command fails, read the error JSON and retry with corrections

## Error Recovery

- "Timer already running" → `keito time stop` first, then start new
- "Project not found" → `keito projects list --json` to find correct slug
- "Authentication failed" → Check `KEITO_API_KEY` env var
- Network error → Entry is auto-queued. Run `keito sync` later.

## Reference Documentation

See `references/` folder for:
- Full command reference
- Common workflow examples
- Error code details
```

---

## Appendix C: Example Agent Workflows

### Workflow 1: OpenClaw Agent Starting a Coding Task

```
Agent receives: "Implement the user settings page for the Acme website project"

1. Agent activates keito-time skill
2. $ keito projects list --json
   → Finds "acme-website" project
3. $ keito projects tasks acme-website --json
   → Finds "development" task
4. $ keito time running --json
   → No timer running
5. $ keito time start --project acme-website --task development \
     --notes "Starting: Implement user settings page"
   → Timer started
6. Agent performs the coding work (creates files, writes code, runs tests)
7. $ keito time stop \
     --notes "Completed: User settings page with profile edit, password change, notification preferences. Files: src/pages/settings.tsx, src/api/settings.ts. Tests: 8 passing."
   → Entry logged: 2.25 hours, billable
```

### Workflow 2: Agent Logging Research Time After the Fact

```
Agent completed 45 minutes of competitive analysis research

1. $ keito time log \
     --project acme-strategy \
     --task research \
     --duration 0.75 \
     --notes "Competitive analysis: Reviewed 5 competitor pricing pages, documented feature comparison matrix" \
     --agent-id openclaw-research-01
   → Entry logged: 0.75 hours, billable
```

### Workflow 3: Agent Session with Multiple Tasks

```
Agent works on two different tasks for the same client in one session

1. $ keito time start --project acme-website --task development \
     --session-id sess_xyz789 --notes "Bug fix: Login timeout issue"
   ... works for 30 minutes ...
2. $ keito time stop --notes "Fixed: Session timeout extended to 30min. File: src/auth/session.ts"
3. $ keito time start --project acme-website --task code-review \
     --session-id sess_xyz789 --notes "Reviewing PR #156: Database migration"
   ... reviews for 20 minutes ...
4. $ keito time stop --notes "Reviewed: Approved PR #156 with 2 suggestions for index optimization"
```

---

*End of PRD — keito-cli v1.0.0*
