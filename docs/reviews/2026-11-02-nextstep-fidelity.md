# NeXTSTEP fidelity: gaps against Lotus Improv 1.0 for NeXT — 2026-11-02

This note ranks what still separates `crates/gui` from the experience of
using Lotus Improv on a NeXT, and records the sources for each point. It
extends `2026-10-03-nextstep-visual-spec.md` (the spec); where the two
disagree, this note cites a primary source and the spec is updated with the
change that lands.

## 1. What "Improv's last NeXT release" is

The brief asked for "Lotus Improv 2.1 on NeXTSTEP 3.x". **There was no
Improv 2.x for NeXT.** Improv 2.0 (1993) and 2.1 (1994) shipped for Windows
3.1 only; the NeXT product stayed at 1.0:

- NeXTWORLD, August 1993, "Most confusing upgrade path: Lotus, for forcing
  Improv 1.0 users to move to Windows in order to use Improv 2.0" [NW93-08].
- NeXTWORLD's catalog pages sell "9794 Improv 1.0" from Fall 1991 through
  April/May 1993 [NW91-F, NW93-04]; NeXTWORLD April 1994 lists only
  "Lotus Improv 1.0J", sold in Japan by Canon [NW94-04].
- The archive.org disk image of the beta notes "the final release of Lotus
  Improv 1.0 is version 108r0, from 1991-01-23" [BETA]; it shipped in
  February 1991 [NW91-F, Garfinkel].
- NeXTWORLD, January 1994: Swiss Bank's "reliance on Improv and
  WordPerfect, two applications whose future on NEXTSTEP for Intel remains
  in doubt" [NW94-01], so it was never ported to NeXTSTEP 3.x for Intel.

**Target: Improv 1.0 on NeXTSTEP 2.x** (2.0 was current when it shipped;
2.1 and 2.2 followed in 1991–92; the same binary ran on 3.x for 68k). Per
the lead's direction, where NeXTSTEP 2.x and 3.x differ in a way a user
sees, this note follows 2.x and says so (§4).

## 2. Sources

Primary, read for this note:

- **[BETA]** *Lotus Improv 1.0 beta (v89.4) + NeXTSTEP 2.0 beta (Warp4.1
  "Magenta")*, archive.org item `improv-beta`, a bootable magneto-optical
  disk image dated 1990-09/10. I read the UFS file system with sleuthkit and
  took **facts only** (menu titles, key equivalents, preference names,
  chart type names, help text) from `Improv.app/Improv` (its compiled
  `MainMenu` nib and string tables), `Improv.app/bbqref.hlp` (the
  quick-reference help book) and `PresentationBuilder.app`. No binary,
  bitmap, icon or font from it is in this repository.
- **[UIG2]** *NEXTstep Concepts*, Release 1, Dec 1990, chapter 2, "The NeXT
  User Interface" (bitsavers
  `pdf/next/Release_1_Dec90/NEXTstep_Concepts_Dec90.pdf`). This is the
  NeXTSTEP 1.0/2.0 user-interface guidelines.
- **[UIG3]** *NeXTSTEP User Interface Guidelines*, Release 3, Nov 1993
  (bitsavers
  `pdf/next/Release_3_Nov93/NeXTSTEP_User_Interface_Guidelines_Release_3_Nov93.pdf`).
- **[REF2]** *NeXTSTEP Reference*, Release 2, Nov 1991 (bitsavers
  `pdf/next/Release_2_Nov91/NeXTSTEP_Reference_Nov91.pdf`): the
  `NX_COLOR*` constants.
- **[NW91-JF]** NeXTWORLD vol. 1 no. 1, Jan/Feb 1991, Robert Lauriston,
  spreadsheet review of PowerStep, Wingz and Improv 1.0 (beta), pp. 59–64.
- **[NW91-F]** NeXTWORLD vol. 1 no. 3, Fall 1991: Simson Garfinkel,
  "Improv: The Inside Story", pp. 33–35, 79 (also simson.net
  `clips/1991/1991.NextWorld.Improv.pdf`); and the "Color Options" sidebar,
  p. 27: "Improv supports color in its whiz-bang charts and graphs."
- **[NW93-08]**, **[NW93-04]**, **[NW94-01]**, **[NW94-04]**: NeXTWORLD
  issues on archive.org (`NeXTWORLDVol.3No.4August1993` and so on).
- The four reference screenshots in `refs/` (W, F, P, N; see the spec).

Secondary: Wikipedia, "Lotus Improv" (history only); GNUstep `NSColor.m`
and Apple's `NSColor` documentation for the RGB values of the named colors
that NeXTSTEP's `NX_COLOR*` constants became (OpenStep kept the same
values).

Not found or not readable: the *Lotus Improv User's Guide* for NeXT (no
scan found on archive.org or bitsavers); *Improv 2.1 Revealed!* (Windows,
archive.org lending only); any 1991–94 screenshot of an Improv **chart**;
a runnable Improv (the beta image needs the Previous emulator and a 68030
setup, not attempted in this time box).

## 3. Verified facts

### 3.1 Main menu (from [BETA]'s MainMenu nib)

Top level: **Info, File, Edit, Format, Worksheet, View, Graph, Print…,
Services, Hide, Quit** (the reference W shows the same). Submenus, with
their key equivalents (Command-*key*; capital = Command-Shift):

| Menu | Items (key) |
| --- | --- |
| Info | Info Panel…, Context Help… (?), Point for Help, Preferences… |
| File | Open… (o), New (n), Save (s), Save As… (S), Revert to Saved, Model Browser…, Close (W) |
| Edit | Cut (x), Copy (c), Copy as Graphic, Paste Items & Cells (v), Paste Special…, Add (+), Delete Views & Graphs, Inspector…, Select All (a), Select Items Only ([), Select Cells Only (]) |
| Format | Format Panel… (F), Font Panel… (t), Bold (b), Italic (i), Larger, Smaller, Report Layout…, Add Page Break, Clear Page Break, Page Layout… |
| Worksheet | New Worksheet (N), Group Items (g), Ungroup Items (G), Add Group Summary…, Data Fill…, Sort Items…, Sort Items By Name…, Check Formula (k), Resolve Overlaps…, Recalculate (r) |
| View | New View (V), Hide Selection, Show Selection, Show All, Collapse Group (<), Expand Group (>), Mark Cells (m), Swap Rows/Columns (/) |
| Graph | New Graph, Add Graph, Replace Graph, Add Series to Graph |
| (top) | Print… (p), Hide (h), Quit (q) |

The menu-item labels (Add, Delete, Paste) change with the selection: the
string table holds "Add Category / Add Formula / Add Item", "Delete Item /
Delete Formula / Delete Cell Value / Delete View & Graph", "Paste Items &
Cells / Paste Text / Paste Formulas / Paste Worksheets".

Improv 1.0 had **no Undo** ([NW91-JF]: "it lacks such basic important
features as an undo command and a macro language"); ours is an allowed
modernization (spec §6).

### 3.2 Graphs

- Types (from [BETA]'s strings, in order): **Line, Bar, Scatter, Pie, Area,
  Stock, 3DBar**; Presentation Builder names them "line, bar, pie,
  scatter, HLOC, area, mixed, three-D bar". [NW91-JF]: "the standard range
  of chart types, plus 3D bars."
- Data is parsed **ByRows, ByColumns or Smart**. The help book, *Preferences
  Panel: Graphing*: "Each row of data selected is a series / Each column of
  data selected is a series / Automatically determines whether rows or
  columns are interpreted as a series … Smart Graph Selection: if a
  selection is one row, then each column of data is a series; if a
  selection is one column, then each row of data is a series; otherwise,
  the longest of the rows or columns becomes the data series."
- The default type is a preference (`DefaultChartType`); the help book's
  example: "This graph's default graph type is Bar. The labels come from
  the items in the data series. The default window size on the screen is
  5.5 x 4.0 inches. The axis scales use the minimum and maximum values
  from the [data]."
- A graph lives in its own **graph window** (Graph ▸ New Graph), drawn by
  the separate **Presentation Builder** application, hot-linked to the
  worksheet: "Changes to the data are immediately reflected in the graph
  for that series."
- Inspectors: **Graph Inspector, Axis Inspector, Series Inspector**
  (`ChartInsp.nib`, `AxisInsp.nib`, `SeriesInsp.nib`); a series has Text
  Color, Line Color and Fill Color, set through the NeXT Colors panel
  (`orderFrontNXColorPanel:`); missing data "Leave Gap / Interpolate";
  scale "In 1,000,000's … In 1's"; a `shufflePieWedgeColors` action.
- Fills, from its tools palette: **black, dark gray, light gray, white**,
  ten patterns and none; the pattern set includes RightDiag, LeftDiag,
  Horiz, Vert, their Light variants, Crosshatch, Crosshatch2.
- Color: "Improv supports color in its whiz-bang charts and graphs"
  [NW91-F, p. 27], on the NeXTstation Color and NeXTdimension (1991).

**Unverified: the exact default series colors** on a color NeXT. No
screenshot of an Improv chart in color was found. What is known: the
monochrome defaults are NeXT's four grays and the patterns above, and the
color picker is NeXT's standard Colors panel, whose named colors are the
`NX_COLOR*` set [REF2]: black, blue, brown, clear, cyan, dark gray, gray,
green, light gray, magenta, orange, purple, red, white, yellow. Our color
palette is therefore built from those named colors (§5, gap 1), and is
marked as a reconstruction.

### 3.3 Window chrome ([UIG2] ch. 2)

- "The title bar is **21 pixels high** and is separated from the content
  area by a one-pixel line that matches the window border." Border 1 px
  black. (The spec chose 23 *(AppKit)*; the 2.x guideline says 21.)
- Resize bar: "**eight pixels high**. The top row of pixels is dark gray,
  the row beneath is white, and the remaining six rows are light gray."
  Dragging the middle region changes the height; "beginning at either end
  … and dragging to the left or right causes the window to become wider or
  narrower"; diagonally moves a corner. The outline follows the pointer,
  then the window resizes on release.
- Title bar buttons: **miniaturize at the left, close at the right**; the
  close button is drawn **broken** while the document has unsaved edits.
- Key window: black title bar. **Main window that is not key: dark gray
  title bar**; other windows light gray. "If the main window is also the
  key window, it has only the black highlighting."
- Menus: "The title bar of a menu is solid black. The title itself isn't
  centered … it begins at the left margin." Menus "have a dark gray
  border just on their top and left sides."
- Highlighting: "Buttons and menu commands have a light gray background
  that turns to white when they're highlighted. Selectable text is
  displayed against a white background, which becomes light gray to mark
  the selection."
- Disabled commands "have dark gray text (instead of the usual black)"
  [UIG3, Menus].

### 3.4 Scrollers ([UIG2], [UIG3] ch. 7)

- "The vertical scroller is positioned along the **left** edge of the
  opening." "The scroll buttons for both vertical and horizontal scrollers
  occupy the **lower left corner**, where the two scrollers meet."
- Clicking in the bar outside the knob: "The knob **jumps to the location
  of the click**, and the display is adjusted accordingly" (not paging).
- A scroll button scrolls a line; pressed, it repeats.
  **Alternate-click scrolls one viewful**, keeping a line or two of
  overlap. Alternate-drag on the knob is a fine-tuning mode.
- The knob never gets smaller than a square. With nothing to scroll the
  knob and buttons disappear and a plain gray strip remains [UIG3].

Improv itself departs from this: its worksheet window puts the vertical
scroller on the **right** and the horizontal one along the bottom with its
arrows at the **left** end (W, F, P all show it). We keep Improv's layout,
since it is the application we copy.

### 3.5 Keyboard ([UIG2] "Keyboard Alternatives")

Reserved in 2.x (must be used for these commands, and only for them):
Command-a Select All, -b Bold, -c Copy, -h Hide, -i Italic, -o Open,
-p Print, -q Quit, -s Save, -t Font Panel, -v Paste, -w Close, -x Cut,
-z Undo; plus the customary Command-n New and Command-? Help. 3.x adds
Command-f Find, Command-S Save As and Command-P Page Layout [UIG3].
Improv's own (§3.1) add Command-r Recalculate, Command-k Check Formula,
Command-V New View, Command-m Mark Cells, Command-/ Swap Rows/Columns,
Command-[ and Command-] Select Items/Cells Only.

### 3.6 Panels, Services, Workspace

- Improv 1.0's panels ([BETA] panel list): Info Panel, Preferences
  (Settings, Save, Font, Group Summary, Graphing), Inspector (Grid,
  Layout, Formulas), Format (Number, Alignment, Line Style, Color), Font,
  Group Summary, Data Fill, Sort, Overlap, Page Layout, Report Layout,
  Print, Worksheet Import/Export, Text Import, Paste Special, Model
  Browser.
- NeXTSTEP 2.0 called the services menu **Request** ("Define in Webster",
  "Open from Workspace", Command-=) [UIG2]; 2.1 renamed it **Services**.
  Improv's nib already says **Services**, so we use that.
- The Workspace dock and app icon, the miniwindow (64×64, "a small title
  bar … drawn across the top"), and Services cannot be reproduced inside
  one egui window without NeXT artwork; see §5, "not planned".

### 3.7 Typography

Improv's string table lists **Courier, Helvetica, Ohlfs, Symbol, Times**,
with Helvetica-Bold for titles. Ohlfs was NeXT's screen font for
fixed-width text (Terminal), not used in Improv's grid. Arimo (bundled,
OFL-1.1) is metric-compatible with Helvetica/Arial, so widths match; its
hinting and the absence of NeXT's 2-bit gray antialiasing are the visible
differences. Not changed.

## 4. NeXTSTEP 2.x vs 3.x, decided

| Point | 2.x | 3.x | Follow | Why |
| --- | --- | --- | --- | --- |
| Title bar height | 21 px [UIG2] | 23 px (AppKit 3) | **2.x** | Improv 1.0 shipped on 2.0; the measured W/F captures (≈21) agree |
| Resize bar | 8 px: dark-gray, white, 6 light-gray rows [UIG2] | 9 px beveled with notches | **2.x** | as above |
| Document menu name | "Window" menu (Open, New, Save, Close) [UIG2] | "Document" | **Improv's own: File** | the nib says File |
| Services menu | "Request" (2.0) | "Services" | **Services** | Improv's nib and W say Services |
| Scroller arrows | lower-left, grouped | lower-left, grouped | **Improv's: right / bottom** | the refs show Improv's own layout |
| Colors panel | NXColorPanel (2.0) | NXColorPanel | same | — |

## 5. Gaps, ranked by how much each breaks the period feel

Severity: **A** the first thing a NeXT user notices; **B** visible in
ordinary use; **C** a detail. Effort: S (hours), M (a day), L (more).

| # | Gap | Sev | Effort | Status |
| --- | --- | --- | --- | --- |
| 1 | **Charts are grayscale-only and sit in a docked egui side panel with a modern "line" checkbox.** Improv drew color charts (Line, Bar, Scatter, Pie, Area, Stock, 3D Bar) in a NeXT graph window, from the selection, parsed by rows, columns or Smart. Fix: a NeXT `Graph` window (title `<view> - Graph`), the chart types, a NeXT color palette from the `NX_COLOR*` set with the four grays as the monochrome fallback, and a small Graph Inspector (type, parse by). | A | M | done: `feat(gui): color graphs in a NeXT graph window` |
| 2 | **Menus and key equivalents do not match Improv's.** Our Worksheet menu holds Pivot, Add Matrix, Save View…; Improv's View menu has Swap Rows/Columns (Command-/) and New View (Command-V), Worksheet has Recalculate (Command-r), Graph has New Graph. Fix: rename and regroup to Improv's labels and keys (§3.1) without dropping a command; keep the modern extras at the ends of their menus. | A | S | done: `feat(gui): Lotus Improv 1.0's menus and key equivalents` |
| 3 | **Title bar 23 px and resize bar 9 px notched at thirds** (spec, AppKit 3.x). 2.x: 21 px, and 8 px of dark-gray, white and light-gray rows, notched near each end where the corner zones end (Figure 2-9). | B | S | done: `feat(gui): NeXTSTEP 2 window chrome` |
| 4 | **No miniaturize button** at the left of the title bar; no **broken close button** for unsaved edits (spec §6 asked for it; not built). | B | S | done: `feat(gui): NeXTSTEP's key, main and other title bars`; miniaturize minimizes the native window (no miniwindows on the workspace) |
| 5 | **Main-window title bar.** A worksheet that lost the key to a panel (Info, Functions) should turn dark gray, not light gray; other windows light gray ([UIG2]). Today any non-key window is dark gray. | B | S | done: same commit |
| 6 | **Menu title and border.** Menu titles start at the left margin (ours do), the border is dark gray on top and left only [UIG2]; ours is black all round. | C | S | done: `feat(gui): the NeXTSTEP 2 menu frame` |
| 7 | **Scroller trough click pages by a view**; NeXT jumps the knob to the click. Alternate-click on an arrow should scroll a viewful. | B | S | done: `feat(gui): NeXT scroller clicks` |
| 8 | **Key equivalents missing** for commands we have: Close (Command-W in Improv; we have no Close), Info Panel has none (correct), Copy/Paste of cells (Command-c/v) unbuilt. | B | M | Copy/Paste done: `feat(gui): Edit > Copy and Paste`; Cut and Close: later |
| 9 | **Inspector and Format panel**: Improv's Edit ▸ Inspector… (Grid, Layout, Formulas) and Format ▸ Format Panel… (Number, Alignment, Line Style, Color) are a multiform panel with a pop-up selector. Ours: the Info panel and the Format submenu. | B | M | done: `feat(gui): the Inspector and the Format panel`; the pages with state to set here (Grid, the Hidden Items marker, Show Errors In Line; Number, Alignment). Not built: recalculation and iteration controls, the Collapsed Groups and Page Breaks markers, Line Style, Color, custom and date/time formats |
| 10 | **Formula pane status indicators**: Improv shows Calc, circular-reference and formula-mismatch indicators (`TWI_Calc`, `TWI_Circ`, `TWI_FMerr`) by the Calc button. | C | S | Checked and Error indicators per row done: `feat(gui): formula status indicators`; Overlap and Mismatch have no engine state to show |
| 11 | **Print and Page Layout**: grayed out. Improv printed views with a Report Layout (headers, footers, page breaks). | B | L | later |
| 12 | **Help**: Improv had context help (Command-? then click). Ours: Help ▸ Keys…, Functions…, User Guide. | C | M | later |
| 13 | **Selection rules**: Improv's Select Items Only / Cells Only (Command-[ / ]) and structured selection of items. Our `[` `]` page instead. | C | M | later |
| 14 | **Cursors**: NeXT's arrow, I-beam and Improv's row/column resize and point-mode cursors (`RowResizeCursor`, `ColResizeCursor`, `PMCursor`). We use egui's system cursors. Drawing our own would need custom cursor images, which winit supports per platform only. | C | M | not planned |
| 15 | **Sounds**: NeXT beeps when a key window does nothing with typing [UIG2]. We are silent. | C | S | not planned (no audio dependency) |
| 16 | **Workspace, dock, app icon, miniwindows, Services**: outside one native window; would need NeXT artwork or OS integration. | C | L | not planned |

## 6. Accessibility and the keyboard

Every change keeps AccessKit labels on painted controls (the existing
`every_interactive_widget_has_an_accesskit_label` test covers the graph
window too) and keeps every command reachable by Tab and by its key
equivalent. **Ctrl stands in for Command** on Linux, FreeBSD, illumos and
Windows; Cmd on macOS. The mapping is in `docs/src/gui-tour.md`,
"Keyboard and screen readers", and in Help ▸ Keys….

## 7. Still unverified

- The default chart colors on a color NeXT (§3.2).
- The look of the Graph, Axis and Series inspectors (names only).
- Whether Improv drew a broken close button; it is the AppKit behavior, and
  Improv set `setDocEdited:` (string present in [BETA]), so very likely.
- A pixel match: still needs a running Improv 1.0 (the Previous emulator
  with the [BETA] image, 68030 mode).
