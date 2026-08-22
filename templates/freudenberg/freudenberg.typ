// ============================================================================
// Freudenberg Group brand template for documents (Typst), self-contained.
//
// The real corporate sans is TheSans, a commercial typeface that must never be
// embedded here. Source Sans 3 is the reviewed open substitute. Roboto Slab is
// the established accent face used for pull-quotes. See ../README.md.
//
// Requires Typst 0.13+, Source Sans 3, and Roboto Slab.
// ============================================================================

// --- Brand palette ----------------------------------------------------------
#let freudenberg-blue = rgb("#004388")
#let freudenberg-cyan = rgb("#00A6E2")
#let freudenberg-yellow = rgb("#F7A600")
#let freudenberg-green = rgb("#76B82A")
#let freudenberg-dark-green = rgb("#007D4E")
#let freudenberg-red = rgb("#DF342E")
#let freudenberg-grey = rgb("#3F3F3F")
#let freudenberg-light-grey = rgb("#CFCFCF")
#let freudenberg-ground = rgb("#F5FAFA")

#let light-theme = (
  bg: freudenberg-ground,
  surface: white,
  fg: freudenberg-grey,
  muted: rgb("#667580"),
  heading: freudenberg-blue,
  accent: freudenberg-cyan,
  edge: freudenberg-light-grey,
)

// --- Reusable brand elements -----------------------------------------------
// A compact live-vector interpretation of the gradient wing plus wordmark.
// The canonical logo artwork remains in ../assets/logo.svg; this library stays
// asset-free so it can be vendored as one reviewed source file.
#let freudenberg-mark(height: 10pt) = box(
  height: height,
  grid(
    columns: (height * 0.65, height * 0.65, height * 0.65),
    column-gutter: height * 0.13,
    align: bottom,
    rotate(-28deg, rect(width: 100%, height: 54%, fill: rgb("#5CC0EB"), radius: 1pt)),
    rotate(-28deg, rect(width: 100%, height: 78%, fill: freudenberg-cyan, radius: 1pt)),
    rotate(-28deg, rect(width: 100%, height: 100%, fill: freudenberg-blue, radius: 1pt)),
  ),
)

#let freudenberg-logo(size: 16pt, compact: false) = grid(
  columns: (auto, auto),
  column-gutter: 8pt,
  align: (left + horizon, left + horizon),
  freudenberg-mark(height: size * 0.82),
  [
    #text(
      font: "Source Sans 3",
      weight: 700,
      size: size,
      tracking: 0.35pt,
      fill: freudenberg-blue,
      [FREUDENBERG],
    )
    #if not compact {
      linebreak()
      text(
        font: "Source Sans 3",
        weight: 500,
        size: size * 0.38,
        tracking: 1.25pt,
        fill: freudenberg-blue,
        [INNOVATING TOGETHER],
      )
    }
  ],
)

#let freudenberg-rule(width: 30mm) = line(
  length: width,
  stroke: 2.2pt + freudenberg-cyan,
)

#let freudenberg-note(title: none, tone: "blue", body) = {
  let colour = if tone == "red" {
    freudenberg-red
  } else if tone == "green" {
    freudenberg-dark-green
  } else if tone == "yellow" {
    freudenberg-yellow
  } else {
    freudenberg-cyan
  }
  block(
    width: 100%,
    breakable: false,
    fill: white,
    stroke: (left: 3pt + colour, rest: 0.5pt + freudenberg-light-grey),
    inset: (x: 13pt, y: 11pt),
    above: 1.2em,
    below: 1.2em,
    {
      if title != none {
        text(font: "Source Sans 3", weight: 700, fill: freudenberg-blue, title)
        v(4pt)
      }
      set text(font: "Source Sans 3", size: 9.5pt, fill: freudenberg-grey)
      set par(leading: 0.78em)
      body
    },
  )
}

#let freudenberg-stat(value, label, accent: freudenberg-cyan) = block(
  width: 100%,
  fill: white,
  stroke: 0.6pt + freudenberg-light-grey,
  inset: 13pt,
  [
    #text(font: "Source Sans 3", weight: 700, size: 24pt, fill: accent, value)
    #v(3pt)
    #text(font: "Source Sans 3", weight: 500, size: 8.5pt, fill: freudenberg-grey, label)
  ],
)

#let freudenberg-shot(path, width: 100%) = box(
  width: width,
  clip: true,
  stroke: 0.6pt + freudenberg-light-grey,
  image(path, width: 100%),
)

// --- Document wrapper -------------------------------------------------------
#let freudenberg-doc(
  theme: light-theme,
  title: "Document title",
  subtitle: none,
  recipient: none,
  author: "Freudenberg Group",
  date: datetime.today(),
  org: "Freudenberg Group",
  address: (),
  phone: none,
  email: none,
  web: "www.freudenberg.com",
  legal: (),
  footer-style: "full",
  support: none,
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
      freudenberg-logo(size: 10pt, compact: true),
      text(
        font: "Source Sans 3",
        weight: 600,
        size: 7.5pt,
        tracking: 0.55pt,
        fill: theme.muted,
        upper(confidentiality),
      ),
    )
    v(5pt)
    line(length: 100%, stroke: 0.6pt + freudenberg-cyan)
  }
  let running-footer = context {
    if counter(page).get().first() <= 1 { return }
    line(length: 100%, stroke: 0.5pt + theme.edge)
    v(6pt)
    if simple {
      let contact = (support, web).filter(x => x != none and x != "").join(" · ")
      grid(
        columns: (1fr, auto),
        align: (left + horizon, right + horizon),
        text(font: "Source Sans 3", size: 7.8pt, fill: theme.muted, contact),
        page-number,
      )
    } else {
      let col-a = (text(weight: 700, fill: theme.heading, org),) + address
      let col-b = (phone, email, web).filter(x => x != none and x != "")
      let rows = calc.max(col-a.len(), col-b.len(), legal.len())
      grid(
        columns: (1.15fr, 1fr, 1fr, auto),
        column-gutter: 10pt,
        row-gutter: 2.5pt,
        align: (left + top, left + top, left + top, right + top),
        ..range(rows)
          .map(i => (
            col-a.at(i, default: []),
            col-b.at(i, default: []),
            legal.at(i, default: []),
            if i == 0 { page-number } else { [] },
          ))
          .flatten(),
      )
    }
  }

  set document(title: title, author: author)
  set page(
    paper: "a4",
    fill: theme.bg,
    margin: (left: 27mm, right: 27mm, top: 27mm, bottom: if simple { 24mm } else { 31mm }),
    header: running-header,
    footer: running-footer,
    header-ascent: 13mm,
    footer-descent: 10mm,
  )
  set text(
    font: "Source Sans 3",
    weight: 400,
    size: 10.3pt,
    fill: theme.fg,
    lang: "en",
    hyphenate: true,
    costs: (widow: 800%, orphan: 800%, runt: 300%),
  )
  set par(justify: true, leading: 0.82em, spacing: 1.15em)
  set heading(numbering: "I.a.1", outlined: true)
  set list(marker: text(fill: freudenberg-cyan)[▪], indent: 1.2em, body-indent: 0.65em)
  set enum(numbering: "1.", indent: 1.2em, body-indent: 0.65em)
  set quote(block: true, quotes: false)
  show link: set text(fill: freudenberg-cyan)
  show strong: set text(weight: 700, fill: freudenberg-blue)
  show quote.where(block: true): it => block(
    width: 100%,
    inset: (left: 14pt, right: 8pt, y: 9pt),
    stroke: (left: 3pt + freudenberg-cyan),
    fill: white,
    text(font: "Roboto Slab", weight: 500, size: 12pt, fill: freudenberg-blue, it.body),
  )
  show heading.where(level: 1): it => {
    pagebreak(weak: true)
    block(above: 0.5em, below: 1.25em, [
      #text(
        font: "Source Sans 3",
        weight: 700,
        size: 9pt,
        fill: freudenberg-cyan,
        tracking: 0.8pt,
        [SECTION #counter(heading).display("I")],
      )
      #v(7pt)
      #text(font: "Source Sans 3", weight: 700, size: 27pt, fill: freudenberg-blue, upper(it.body))
      #v(9pt)
      #freudenberg-rule()
    ])
  }
  show heading.where(level: 2): it => block(above: 1.5em, below: 0.72em, [
    #text(font: "Source Sans 3", weight: 600, size: 16pt, fill: freudenberg-cyan, it.body)
  ])
  show heading.where(level: 3): it => block(above: 1.1em, below: 0.5em, [
    #text(font: "Source Sans 3", weight: 600, size: 11.5pt, fill: freudenberg-blue, it.body)
  ])

  // Cover: off-white ground, exact two-blue hierarchy, restrained secondary colour.
  page(
    fill: freudenberg-ground,
    margin: (left: 28mm, right: 28mm, top: 24mm, bottom: 26mm),
    header: none,
    footer: none,
    {
      freudenberg-logo(size: 17pt)
      place(right + top, dx: 20mm, dy: -16mm, circle(
        radius: 50mm,
        stroke: 12pt + freudenberg-cyan.transparentize(82%),
      ))
      v(1fr)
      block(width: 84%, [
        #text(
          font: "Source Sans 3",
          weight: 900,
          size: 35pt,
          tracking: 0.15pt,
          fill: freudenberg-blue,
          upper(title),
        )
        #if subtitle != none {
          v(11pt)
          text(font: "Source Sans 3", weight: 300, size: 15pt, fill: freudenberg-grey, subtitle)
        }
        #v(18pt)
        #freudenberg-rule(width: 34mm)
      ])
      v(1fr)
      grid(
        columns: (1fr, 1fr),
        column-gutter: 18pt,
        align: (left + bottom, left + bottom),
        [
          #text(font: "Source Sans 3", size: 7.5pt, weight: 700, fill: freudenberg-cyan,
            tracking: 0.7pt, [PREPARED BY])
          #v(4pt)
          #text(font: "Source Sans 3", size: 10pt, fill: freudenberg-grey, author)
          #if recipient != none {
            v(10pt)
            text(font: "Source Sans 3", size: 7.5pt, weight: 700, fill: freudenberg-cyan,
              tracking: 0.7pt, [PREPARED FOR])
            v(4pt)
            text(font: "Source Sans 3", size: 10pt, fill: freudenberg-grey, recipient)
          }
        ],
        [
          #text(font: "Source Sans 3", size: 7.5pt, weight: 700, fill: freudenberg-cyan,
            tracking: 0.7pt, [DATE])
          #v(4pt)
          #text(
            font: "Source Sans 3",
            size: 10pt,
            fill: freudenberg-grey,
            date.display("[day padding:zero] [month repr:long] [year]"),
          )
        ],
      )
    },
  )

  if contents {
    pagebreak()
    text(font: "Source Sans 3", weight: 700, size: 25pt, fill: freudenberg-blue)[CONTENTS]
    v(8pt)
    freudenberg-rule()
    v(18pt)
    show outline.entry: it => block(below: 8pt, it)
    outline(title: none, depth: 2, indent: auto)
  }

  pagebreak()
  body
}
