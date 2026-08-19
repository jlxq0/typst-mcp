// ============================================================================
// Lenno brand template for documents (Typst), self-contained and asset-free.
//
// Dark, modern and technical: a near-black ground, crisp type and one warm
// orange accent. Space Grotesk carries the wordmark and headings; Roboto carries
// body copy. The wordmark is live text because Lenno has no separate logo asset.
// See ../README.md for the source brief.
//
// Requires Typst 0.13+ and the Space Grotesk and Roboto font families.
//
// Usage:
//   #import "lenno.typ": *
//   #show: lenno-doc.with(title: "...", date: datetime(...))
//   light mode:     #show: lenno-doc.with(theme: light-theme, ...)
//   simple footer:  #show: lenno-doc.with(footer-style: "simple", ...)
//   no contents:    #show: lenno-doc.with(contents: false, ...)
// ============================================================================

// --- Brand palette ----------------------------------------------------------
#let lenno-orange = rgb("#F97316")
#let lenno-orange-hover = rgb("#FB923C")
#let lenno-dark = rgb("#0D1218")
#let lenno-dark-surface = rgb("#1A2028")
#let lenno-dark-raised = rgb("#222A33")
#let lenno-dark-edge = rgb("#2E3740")
#let lenno-dark-text = rgb("#E2E8F0")
#let lenno-dark-muted = rgb("#94A3B8")
#let lenno-light = rgb("#F5F3F0")
#let lenno-light-surface = rgb("#EDEAE5")
#let lenno-light-card = rgb("#FDFCFA")
#let lenno-light-edge = rgb("#E8E4DF")
#let lenno-light-text = rgb("#1E293B")
#let lenno-light-muted = rgb("#64748B")
#let lenno-success = rgb("#16A34A")
#let lenno-error = rgb("#DC2626")
#let lenno-info = rgb("#3A7A8C")

#let dark-theme = (
  bg: lenno-dark,
  surface: lenno-dark-surface,
  raised: lenno-dark-raised,
  edge: lenno-dark-edge,
  fg: lenno-dark-text,
  muted: lenno-dark-muted,
  accent: lenno-orange,
)

#let light-theme = (
  bg: lenno-light,
  surface: lenno-light-surface,
  raised: lenno-light-card,
  edge: lenno-light-edge,
  fg: lenno-light-text,
  muted: lenno-light-muted,
  accent: lenno-orange,
)

// --- Reusable brand elements -----------------------------------------------
#let lenno-wordmark(theme: dark-theme, size: 22pt) = text(
  font: "Space Grotesk",
  weight: 650,
  size: size,
  fill: theme.fg,
  tracking: -0.35pt,
)[Lenno#text(fill: theme.accent)[.]]

#let lenno-rule(theme: dark-theme, width: 22mm) = line(
  length: width,
  stroke: 2.2pt + theme.accent,
)

#let lenno-note(
  title: none,
  tone: "accent",
  theme: dark-theme,
  body,
) = {
  let colour = if tone == "success" {
    lenno-success
  } else if tone == "error" {
    lenno-error
  } else if tone == "info" {
    lenno-info
  } else {
    theme.accent
  }
  block(
    width: 100%,
    breakable: false,
    fill: theme.surface,
    stroke: (left: 2.5pt + colour, rest: 0.65pt + theme.edge),
    inset: (x: 13pt, y: 11pt),
    radius: 3pt,
    above: 1.2em,
    below: 1.2em,
    {
      if title != none {
        text(font: "Space Grotesk", weight: 650, fill: theme.fg, title)
        v(4pt)
      }
      set text(font: "Roboto", size: 9.5pt, fill: theme.fg)
      set par(leading: 0.78em)
      body
    },
  )
}

#let lenno-stat(value, label, theme: dark-theme) = block(
  width: 100%,
  fill: theme.surface,
  stroke: 0.65pt + theme.edge,
  inset: 13pt,
  radius: 3pt,
  [
    #text(font: "Space Grotesk", weight: 650, size: 24pt, fill: theme.accent, value)
    #v(3pt)
    #text(font: "Roboto", weight: 500, size: 8.5pt, fill: theme.muted, label)
  ],
)

// --- Document wrapper -------------------------------------------------------
#let lenno-doc(
  theme: dark-theme,
  title: "Document title",
  subtitle: none,
  recipient: none,
  author: "Lenno",
  date: datetime.today(),
  org: "Lenno",
  email: "hello@lenno.ai",
  web: "lenno.ai",
  footer-style: "full",
  support: "support@lenno.ai",
  confidentiality: "Confidential",
  contents: true,
  body,
) = {
  let simple = footer-style == "simple"
  let page-number = context {
    let current = counter(page).get().first()
    let total = counter(page).final().first()
    text(fill: theme.muted, size: 8pt)[#current / #total]
  }
  let running-header = context {
    if counter(page).get().first() <= 1 { return }
    grid(
      columns: (1fr, auto),
      align: (left + horizon, right + horizon),
      lenno-wordmark(theme: theme, size: 12pt),
      text(font: "Roboto", size: 7.5pt, fill: theme.muted, upper(confidentiality)),
    )
    v(5pt)
    line(length: 100%, stroke: 0.55pt + theme.edge)
  }
  let running-footer = context {
    if counter(page).get().first() <= 1 { return }
    line(length: 100%, stroke: 0.55pt + theme.edge)
    v(6pt)
    if simple {
      grid(
        columns: (1fr, auto),
        align: (left + horizon, right + horizon),
        text(font: "Roboto", size: 8pt, fill: theme.muted)[#support · #web],
        page-number,
      )
    } else {
      grid(
        columns: (1fr, 1fr, auto),
        column-gutter: 12pt,
        align: (left + top, left + top, right + top),
        [#text(font: "Space Grotesk", weight: 650, size: 8pt, fill: theme.fg, org) \
         #text(font: "Roboto", size: 7.5pt, fill: theme.muted, confidentiality)],
        [#text(font: "Roboto", size: 7.5pt, fill: theme.muted)[#email \
         #web]],
        page-number,
      )
    }
  }

  set document(title: title, author: author)
  set page(
    paper: "a4",
    fill: theme.bg,
    margin: (left: 25mm, right: 25mm, top: 25mm, bottom: 24mm),
    header: running-header,
    footer: running-footer,
  )
  set text(font: "Roboto", size: 10.2pt, fill: theme.fg)
  set par(justify: true, leading: 0.82em)
  set heading(numbering: "I.a.1", outlined: true)
  set list(marker: text(fill: theme.accent)[•], indent: 1.2em, body-indent: 0.65em)
  set enum(numbering: "1.", indent: 1.2em, body-indent: 0.65em)
  set quote(block: true, quotes: false)
  show link: set text(fill: theme.accent)
  show strong: set text(font: "Space Grotesk", weight: 650)
  show raw: set text(font: "Roboto", size: 8.8pt, fill: theme.fg)
  show quote.where(block: true): it => block(
    width: 100%,
    inset: (left: 13pt, right: 6pt, y: 8pt),
    stroke: (left: 2pt + theme.accent),
    fill: theme.surface,
    text(font: "Space Grotesk", weight: 450, size: 12pt, fill: theme.fg, it.body),
  )
  show heading.where(level: 1): it => {
    pagebreak(weak: true)
    block(above: 0.5em, below: 1.35em, [
      #text(font: "Space Grotesk", weight: 650, size: 9pt, fill: theme.accent,
        tracking: 0.8pt, [SECTION #counter(heading).display("I")])
      #v(7pt)
      #text(font: "Space Grotesk", weight: 650, size: 26pt, fill: theme.fg, it.body)
      #v(9pt)
      #lenno-rule(theme: theme)
    ])
  }
  show heading.where(level: 2): it => block(above: 1.5em, below: 0.75em, [
    #text(font: "Space Grotesk", weight: 650, size: 15pt, fill: theme.fg, it.body)
  ])
  show heading.where(level: 3): it => block(above: 1.2em, below: 0.55em, [
    #text(font: "Space Grotesk", weight: 600, size: 11pt, fill: theme.accent, it.body)
  ])

  // Full-bleed cover. Orange appears only as the wordmark dot, rule and small
  // metadata labels, preserving the intentionally restrained brand system.
  page(
    fill: lenno-dark,
    margin: (left: 28mm, right: 28mm, top: 25mm, bottom: 28mm),
    header: none,
    footer: none,
    {
      set text(fill: lenno-dark-text)
      lenno-wordmark(theme: dark-theme, size: 20pt)
      v(1fr)
      block(width: 86%, [
        #block(text(font: "Space Grotesk", weight: 650, size: 34pt,
          tracking: -0.6pt, title))
        #if subtitle != none {
          v(12pt)
          text(font: "Roboto", weight: 300, size: 14pt, fill: lenno-dark-muted, subtitle)
        }
        #v(18pt)
        #lenno-rule(theme: dark-theme, width: 28mm)
      ])
      v(1fr)
      grid(
        columns: (1fr, 1fr),
        column-gutter: 18pt,
        align: (left + bottom, left + bottom),
        [
          #text(font: "Roboto", size: 7.5pt, weight: 500, fill: lenno-orange,
            tracking: 0.7pt, [PREPARED BY])
          #v(4pt)
          #text(font: "Roboto", size: 10pt, fill: lenno-dark-text, author)
          #if recipient != none {
            v(10pt)
            text(font: "Roboto", size: 7.5pt, weight: 500, fill: lenno-orange,
              tracking: 0.7pt, [PREPARED FOR])
            v(4pt)
            text(font: "Roboto", size: 10pt, fill: lenno-dark-text, recipient)
          }
        ],
        [
          #text(font: "Roboto", size: 7.5pt, weight: 500, fill: lenno-orange,
            tracking: 0.7pt, [DATE])
          #v(4pt)
          #text(font: "Roboto", size: 10pt, fill: lenno-dark-text,
            date.display("[day padding:zero] [month repr:long] [year]"))
        ],
      )
    },
  )

  if contents {
    pagebreak()
    text(font: "Space Grotesk", weight: 650, size: 24pt, fill: theme.fg)[Contents#text(fill: theme.accent)[.]]
    v(8pt)
    lenno-rule(theme: theme)
    v(18pt)
    show outline.entry: it => block(below: 8pt, it)
    outline(title: none, depth: 2, indent: auto)
  }

  pagebreak()
  body
}
