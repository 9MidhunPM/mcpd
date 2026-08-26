# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Syncplane is for individual developers who use more than one MCP-capable AI
client. They need the same servers and safe credentials to work in each client
without manually maintaining incompatible configuration files.

## Product Purpose

Syncplane is a local-first open-source Rust CLI that keeps one canonical MCP
configuration and safely projects the explicitly enabled servers into Claude
Code, Cursor, Codex, Antigravity, and OpenCode-compatible clients.

## Positioning

Configure MCP once. Use it everywhere. Syncplane owns only the configuration it
manages, preserves unrelated client settings, and never becomes an MCP proxy,
agent runtime, marketplace, or hosted service.

## Operating Context

Users run Syncplane in a terminal on Linux. The canonical configuration lives
under the XDG configuration directory, optional project overlays require
explicit trust, and local state tracks ownership, drift, and backups. Secrets
stay symbolic until late runtime resolution through the system keyring.

## Capabilities and Constraints

- Linux x86_64 is the supported binary platform for the 1.0.0 release.
- Built-in targets are Claude Code, Cursor, Codex, Antigravity, and
  OpenCode-compatible configuration.
- Sync, dry-run, ownership, atomic writes, backups, diagnostics, watch mode,
  shell completions, and user-level systemd are real product features.
- No raw secrets, fabricated customer proof, adoption metrics, or performance
  claims may appear in public material.

## Brand Commitments

The public name is Syncplane. The product should communicate safety and
technical competence without dark-neon SaaS styling. The approved visual world
is Registration Press: cool paper, process inks, registration marks, and
precise proofing as a metaphor for aligning multiple native configurations.

## Evidence on Hand

The repository provides executable CLI output, source-derived target behavior,
architecture records, and isolated test fixtures. Screenshots must be captured
from deterministic temporary fixtures, not a real home directory or live
customer configuration.

## Product Principles

1. Canonical configuration is the source of truth.
2. Safety outranks convenience: unmanaged entries, secrets, and project trust
   boundaries are protected by default.
3. A no-op sync must remain a no-op.
4. Each target keeps its native format and client-specific behavior.
5. The release must be inspectable: source, checksums, provenance, and tutorial
   commands should agree.

## Accessibility & Inclusion

The website targets WCAG 2.2 AA: semantic reading order, keyboard operation,
visible focus, sufficient contrast, non-color status cues, and reduced motion.
