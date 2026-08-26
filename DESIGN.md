---
name: Syncplane
description: A registration-press website for one canonical MCP environment.
colors:
  paper: "#f2f0e8"
  ink: "#10100f"
  muted-ink: "#65635c"
  process-cyan: "#00a9e0"
  process-magenta: "#ee3f8c"
  process-yellow: "#f4d42e"
  process-red: "#ed3d32"
typography:
  display:
    fontFamily: "Archivo Narrow, Impact, sans-serif"
    fontSize: "clamp(4.25rem, 9.8vw, 10.8rem)"
    fontWeight: 650
    lineHeight: 0.9
    letterSpacing: "-0.04em"
  body:
    fontFamily: "Manrope, Arial, sans-serif"
    fontWeight: 400
    lineHeight: 1.55
rounded:
  none: "0"
spacing:
  compact: "1rem"
  section: "clamp(4rem, 9vw, 10rem)"
components:
  button-primary:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
    rounded: "{rounded.none}"
    padding: "0.9rem 1.05rem"
---

# Design System: Syncplane

## Overview

**Creative North Star: "Registration Press"**

Syncplane looks like a technical proof pulled from a press room: one master
plate registers cleanly against several client-specific sheets. Cool paper and
black ink establish trust; process inks mark alignment, ownership, and action.
The result is editorial and tactile, not a generic dashboard or neon SaaS
landing page.

## Colors

Paper and ink carry most reading; process color is structural evidence, never
the only cue for a state.

### Primary

- **Press Ink** (`#10100f`): headings, primary controls, rules, terminal ground.
- **Proof Paper** (`#f2f0e8`): the default surface and light text on ink.
- **Process Cyan, Magenta, Yellow, Red**: registration plates, direction, and
  focused evidence.

## Typography

**Display Font:** Archivo Narrow with Impact fallback.
**Body Font:** Manrope with Arial fallback.

Display is compressed, decisive, and large. Body copy remains calm and readable
at a 65–75 character measure.

## Layout

Large compositions use asymmetric two-column grids and generous proof margins.
Documentation collapses from a sticky side index to a two-column mobile index.
Section rhythm is deliberate: dense evidence is followed by quiet paper.

## Elevation & Depth

The system is mostly flat. Offset process-color shadows indicate a printed plate
or terminal proof; they never decorate ordinary containers.

## Shapes

Hard rules, square corners, crop marks, circles used only as registration/loupe
devices, and overprinted rectangular plates form the geometry.

## Components

### Buttons

Primary actions are black ink rectangles with paper text and a process-yellow
arrow. Secondary actions are underlined text. Focus uses a cyan outline.

### Navigation

Navigation is a press-header rule with a compact registration wordmark. Mobile
keeps the install action and hides lower-priority links.

### Signature Components

The canonical plate, target plate stack, loupe, and terminal proof make the
product mechanism visible without fictional client logos or marketing metrics.

## Do's and Don'ts

### Do:

- **Do** make Syncplane’s one-to-many configuration mechanism visible early.
- **Do** use actual CLI output and source-derived claims as proof.
- **Do** retain clear keyboard focus and reduced-motion fallbacks.

### Don't:

- **Don't** use glass, gradient text, generic icon-card grids, or dark-neon UI.
- **Don't** present process color as the only meaning-bearing signal.
- **Don't** fabricate benchmarks, customer stories, screenshots, or capabilities.
