---
name: Obsidian Kinetic HUD
colors:
  surface: '#0d141d'
  surface-dim: '#0d141d'
  surface-bright: '#333a44'
  surface-container-lowest: '#080f18'
  surface-container-low: '#151c26'
  surface-container: '#19202a'
  surface-container-high: '#242a34'
  surface-container-highest: '#2e353f'
  on-surface: '#dce3f0'
  on-surface-variant: '#b9cacb'
  inverse-surface: '#dce3f0'
  inverse-on-surface: '#2a313b'
  outline: '#849495'
  outline-variant: '#3b494b'
  surface-tint: '#00dbe9'
  primary: '#dbfcff'
  on-primary: '#00363a'
  primary-container: '#00f0ff'
  on-primary-container: '#006970'
  inverse-primary: '#006970'
  secondary: '#ffc384'
  on-secondary: '#482900'
  secondary-container: '#fe9d00'
  on-secondary-container: '#663c00'
  tertiary: '#f4f5ff'
  on-tertiary: '#002e6a'
  tertiary-container: '#cbd9ff'
  on-tertiary-container: '#005ac4'
  error: '#ffb4ab'
  on-error: '#690005'
  error-container: '#93000a'
  on-error-container: '#ffdad6'
  primary-fixed: '#7df4ff'
  primary-fixed-dim: '#00dbe9'
  on-primary-fixed: '#002022'
  on-primary-fixed-variant: '#004f54'
  secondary-fixed: '#ffdcbb'
  secondary-fixed-dim: '#ffb869'
  on-secondary-fixed: '#2c1700'
  on-secondary-fixed-variant: '#673d00'
  tertiary-fixed: '#d8e2ff'
  tertiary-fixed-dim: '#aec6ff'
  on-tertiary-fixed: '#001a42'
  on-tertiary-fixed-variant: '#004396'
  background: '#0d141d'
  on-background: '#dce3f0'
  surface-variant: '#2e353f'
typography:
  display-lg:
    fontFamily: Space Grotesk
    fontSize: 56px
    fontWeight: '700'
    lineHeight: 64px
    letterSpacing: 0.08em
  display-sm:
    fontFamily: Space Grotesk
    fontSize: 36px
    fontWeight: '600'
    lineHeight: 44px
    letterSpacing: 0.06em
  headline-lg:
    fontFamily: Space Grotesk
    fontSize: 28px
    fontWeight: '600'
    lineHeight: 36px
    letterSpacing: 0.04em
  headline-md:
    fontFamily: Space Grotesk
    fontSize: 22px
    fontWeight: '500'
    lineHeight: 30px
    letterSpacing: 0.03em
  headline-sm:
    fontFamily: JetBrains Mono
    fontSize: 16px
    fontWeight: '700'
    lineHeight: 24px
    letterSpacing: 0.05em
  body-lg:
    fontFamily: JetBrains Mono
    fontSize: 15px
    fontWeight: '400'
    lineHeight: 22px
    letterSpacing: 0.01em
  body-md:
    fontFamily: JetBrains Mono
    fontSize: 13px
    fontWeight: '400'
    lineHeight: 18px
    letterSpacing: 0.0em
  body-sm:
    fontFamily: JetBrains Mono
    fontSize: 11px
    fontWeight: '300'
    lineHeight: 16px
    letterSpacing: 0.02em
  label-lg:
    fontFamily: JetBrains Mono
    fontSize: 12px
    fontWeight: '600'
    lineHeight: 16px
    letterSpacing: 0.1em
  label-md:
    fontFamily: JetBrains Mono
    fontSize: 10px
    fontWeight: '500'
    lineHeight: 14px
    letterSpacing: 0.12em
  label-sm:
    fontFamily: JetBrains Mono
    fontSize: 9px
    fontWeight: '500'
    lineHeight: 12px
    letterSpacing: 0.16em
rounded:
  sm: 0.125rem
  DEFAULT: 0.25rem
  md: 0.375rem
  lg: 0.5rem
  xl: 0.75rem
  full: 9999px
spacing:
  gutter: 1rem
  margin: 1.5rem
  space-xs: 0.25rem
  space-sm: 0.5rem
  space-md: 1rem
  space-lg: 1.5rem
  space-xl: 2rem
---

## Brand & Style

This design system translates the command-center density of advanced holographic heads-up displays into an ultra-precise, functional Linux desktop environment. It serves power users, system architects, and technical operators who treat their operating system not as passive software, but as an active cybernetic instrument.

### Design Direction: Holographic Precision Glass
The aesthetic fuses high-contrast tactical sci-fi instrumentation with modern functional glassmorphism. Surfaces are layered deep-void transparencies illuminated by inner light, precise technical callouts, and energetic kinetic feedback loops. 

### Visual Tenets
- **Optical Luminescence:** Contrast is established through focused luminescence against void substrates rather than opaque solid color planes. Information projects forward through controlled light emission.
- **Architectural Telemetry:** Every UI element communicates system state, diagnostics, or operational context. No space is purely decorative; framing motifs serve as coordinate markers, orientation indicators, and signal status monitors.
- **Instrument Precision:** Hairline glass bevels, chamfered corner cuts, micro-grid backdrops, and monospaced telemetry data create an atmosphere of aerospace-grade engineering.

## Colors

The palette is engineered around an absolute void environment where holographic light establishes depth, functional priority, and structural containment.

### Color Architecture
- **Primary (`#00f0ff` / Holographic Cyan):** The primary optical emitter. Represents nominal states, high-priority telemetry, laser reticles, active terminal traces, and active workspace focus.
- **Secondary (`#ff9d00` / Stark Reactor Amber):** Used for power thresholds, process warnings, thermal spikes, hardware interrupts, and active execution triggers.
- **Tertiary (`#0077ff` / Deep Vector Blue):** Structural geometry, passive radar meshes, inactive window perimeters, and low-priority background telemetry lines.
- **Neutral Core (`#050b14` / Void Black & `#081226` / Deep Obsidian):** Deep substrate darkness that absorbs ambient contrast, allowing semi-transparent luminous layers to hover with optical depth.

### Functional Status Tokens
- **Critical / Overload:** `#ff2a55` (Crimson Pulse)
- **Nominal / Link Established:** `#00f0ff` (Primary Luminescence)
- **Warning / Reactor Strain:** `#ff9d00` (Arc Gold)
- **Sub-System Standby:** `#1e3b5c` (Subdued Vector Slate)
- **Glass Panel Surface:** `rgba(8, 18, 38, 0.65)` with backdrop-filter blur `16px`
- **Active Structural Rim:** `rgba(0, 240, 255, 0.35)` with an inner drop shadow glow

## Typography

The typographic balance relies on the tension between computational monospace readouts and angular geometric display titles.

### Hierarchy & Style Rules
- **Display & Section Headers:** Set in `Space Grotesk`, always transformed to uppercase with elevated letter spacing (`letterSpacing >= 0.04em`). Used for HUD mode banners, desktop workspace identifiers, and system alert states.
- **Body & Instrumentation:** Set in `JetBrains Mono`. Monospace alignment ensures dense multi-column telemetry (process IDs, core temperatures, memory addresses, and terminal logs) aligns to a rigid vertical scanline without horizontal jitter.
- **Micro-Labels & Technical Annotations:** All metadata badges, system state markers, and hardware telemetry keys employ `label-sm` or `label-md` with `0.12em - 0.16em` tracking and uppercase styling to mirror avionic instrument panels.

## Layout & Spacing

The desktop layout uses an asymmetric tactical HUD architecture anchored by a 12-column fluid grid. Outer margins leave dynamic clearance for environmental telemetry overlays, holographic coordinate ticks, and edge-docked utility docks.

### Desktop Shell & Workspace Composition
- **Stark OS Top Status Bar:** Fixed 36px top strip housing host kernel strings (`6.1.0-DEBIAN-AMD64`), clock frequencies, system architecture glyphs, and the Stark-integrated Debian spiral badge.
- **Flank Telemetry Docks:** Left and right perimeter channels (spanning 2 to 3 columns) house live CPU/RAM ring meters, thermal histograms, network packet oscilloscopes, and real-time thread dispatchers.
- **Operational Center Canvas:** The central region houses active terminal nodes, system management windows, and holographic viewport projections.

### Responsive Reflow
- **Ultra-Wide / Multi-Monitor (>= 1920px):** Full dual-dock telemetry activated. Center stage accommodates side-by-side terminal splits without occlusion.
- **Standard Desktop / Laptop (1024px - 1919px):** Peripheral telemetry collapses into slide-out micro-collapsible wings or HUD overlay layers toggled via hotkey.
- **Field Terminal / Compact (< 1024px):** Single-column stacked telemetry modules; edge HUD indicators condense into a unified top-strip status summary.

## Elevation & Depth

Visual hierarchy does not use drop shadows cast from an external light source. Instead, elevation is defined through optical emission, focal plane blur, and translucent substrate layering.

### Surface Elevation Layers
- **Substrate Level (Desktop Canvas):** `#050b14` overlaid with an extremely subtle holographic crosshair dot grid (`rgba(0, 240, 255, 0.04)` at 32px increments).
- **Surface Level 1 (Dock & Passive Shells):** `rgba(8, 18, 38, 0.45)` with `12px` backdrop-blur and a hairline stroke of `rgba(0, 119, 255, 0.25)`.
- **Surface Level 2 (Active Windows & Terminal Glass):** `rgba(8, 18, 38, 0.70)` with `20px` backdrop-blur, bounded by an outer stroke of `rgba(0, 240, 255, 0.40)`.
- **Surface Level 3 (Focus States & Critical Diagnostic Modals):** `rgba(10, 25, 50, 0.85)` with `24px` backdrop-blur, an illuminated neon inner border (`inset 0 0 12px rgba(0, 240, 255, 0.15)`), and an external halo glow (`0 0 24px rgba(0, 240, 255, 0.20)`).

### Luminous Depth Accents
Depth is reinforced via corner bracket overlays ("reticle ticks") set at element boundaries: 8px L-shaped markers that glow `#00f0ff` or `#ff9d00` to indicate active pointer capture or system processing.

## Shapes

The geometric language uses precise 45-degree chamfers, micro-filleted structural corners, and segmented cybernetic silhouettes.

### Chamfer Cuts & Radii
- Structural containers use `roundedness: 1` (0.25rem / 4px) for subtle micro-beveling.
- Primary diagnostic panels, active window headers, and terminal title cards incorporate tactical chamfers: diagonal 8px or 12px cuts on diagonally opposing corners (top-right and bottom-left) to establish a distinct military HUD profile.
- Action tags, terminal prompt pills, and process status indicators utilize faceted clip paths rather than smooth standard pill shapes.

## Components

### Buttons & Holographic Triggers
- **Primary Reactor Button:** Translucent `#00f0ff` wash (`rgba(0, 240, 255, 0.12)`) bounded by a 1px solid `#00f0ff` stroke. Typographic label in `label-lg` uppercase. On hover, the inner glass flood escalates to `rgba(0, 240, 255, 0.28)` accompanied by an outer radial halo (`box-shadow: 0 0 16px rgba(0, 240, 255, 0.4)`).
- **Secondary / Emergency Trigger:** Dark obsidian fill with an energetic Stark amber boundary (`#ff9d00`). Hover creates a high-voltage amber flash state with inverse high-contrast text.
- **Terminal Segmented Control:** Interlocked rectangular segments separated by 1px dark hairline gaps, indicating active run-levels, shell sessions, or active workspaces.

### Windows & Glass Panels
- **Anatomy:** 32px integrated header bar featuring a micro-dot matrix pattern, hostname breadcrumb, chamfered corner cuts, and minimal glyph window controls (minimize, scale, terminate).
- **Borders:** Multi-layered stroke: outer hairline `rgba(0, 240, 255, 0.3)`, inner hairline `rgba(255, 255, 255, 0.05)`, with corner tick coordinates (`[+ -]`) rendered at bottom edges.

### Status Indicators, Checkboxes & Switches
- **Telemetry Checkboxes:** Square frames with 45-degree angled inner corners. Active state displays an illuminated cyan core surrounded by a micro-reticle border.
- **HUD Toggle Switch:** Horizontal sliding cell with segmented dual rails. Active toggle slides an amber or cyan plasma-bar into lock position with an audible frequency click profile.
- **System Badges & Chips:** Monospaced micro-chips containing execution states (`SYS_OK`, `KERNEL_PANIC`, `THREAD_SATURATED`) encased in border brackets with pulse-dot status lights.

### HUD Diagnostic Widgets & Meters
- **Debian Core Arc Reactor:** Circular telemetry widget centered around a stylized vector Debian spiral integrated into an Arc Reactor core geometry. Displays real-time aggregate CPU load via multi-segment concentric glowing rings.
- **Waveform Audio & Bus Visualizers:** Vertical bars or continuous line scopes rendered in `#00f0ff` that dynamically modulate line width and glow amplitude based on ALSA/Pulse audio stream levels or I/O throughput.
- **Memory & Process Matrix:** Tabular monospaced list with alternating row luminescence, pinned telemetry headers, and inline dynamic bar gauges showing allocation per process PID.