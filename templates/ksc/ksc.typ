// ============================================================================
// Kampong Social Club brand template for documents (Typst) - self-contained,
// single file.
//
// The KSC counterpart to ../../hanso/ (whose Typst library lives at
// OfficeMaster typst/hanso.typ). It shares that file's *shape* - a single
// wrapper function, embedded single-ink artwork, light + dark themes, a
// full/simple footer switch - and none of its brand: no stripe sweeps, no
// hexagon icons, no Figtree, no Hanso defaults.
//
// Brand: cream ground, near-black ink for all type, one punchy red reserved
// for the logo and small accents. Passion One (heavy condensed display) over
// Inter (body). Photo-forward, generous cream space, friendly tone. Headings
// are ink, never red. No green. No em dashes - use a spaced en dash.
// See ../README.md for the brief and ../../../CONVENTIONS.md for the shared
// authoring rules.
//
// Requires: Typst 0.13+ and the Passion One + Inter families, either installed
// or supplied with `--font-path ../fonts` (both are committed there, OFL).
//
// Usage:
//   #import "ksc.typ": *
//   #show: ksc-doc.with(title: "...", date: datetime(year: .., month: .., day: ..))
//   dark mode:      #show: ksc-doc.with(theme: dark-theme, ...)
//   simple footer:  #show: ksc-doc.with(footer-style: "simple", ...)
//                   -> one line: support address, website, classification, fediverse
//                      handle. The 3-column address / contact / operator block is
//                      the default, for anything that leaves KSC.
//   no contents:    #show: ksc-doc.with(contents: false, ...)   (short documents)
//
//   = Chapter          -> new page, numbered I. II. III., uppercase, Passion One
//   == Section         -> numbered a. b. c.   (=== -> 1. 2. 3., Inter bold)
//   #quote(block: true)[...]                  -> standfirst under a chapter title
//   #ksc-note[...]  /  #ksc-note(title: "..")[..]  -> a called-out box
//   #figure(image(..), caption: [..])         -> numbered "Figure N - ..."
//   #figure(table(..), caption: [..])         -> branded table, "Table N - ..."
// ============================================================================

// --- Brand palette ----------------------------------------------------------
// Hex values are the ones the shipping product actually renders: the web app's
// theme tokens in ksc_web/assets/css/app.css (--color-brand / --color-light /
// --color-dark and the tinted neutral ramp).
//
// Known drift, deliberately not smoothed over here: the red exists in three
// near-identical values across the estate - #EB1105 on the web and in
// transactional email, #EB1104 in the Japan Workation deck that ../README.md
// was written from, and #EB1205 in ksc_ios (Theme.swift). This template follows
// the web token, because that is the one a member sees. Pick one and retire the
// other two before this comment gets any older.
#let ksc-red = rgb("#EB1105") // --color-brand
#let ksc-cream = rgb("#FDFBF8") // --color-light
#let ksc-ink = rgb("#071411") // --color-dark
#let ksc-slate = rgb("#3E4643") // --color-neutral-700: the site's BODY text colour
#let ksc-graphite = rgb("#232D2A") // --color-neutral-900: the site's heading colour
#let ksc-charcoal = rgb("#2D3532") // --color-neutral-800
// Headings sit halfway between neutral-900 and --color-dark: 16.2:1 on the cream
// where those two give 13.7:1 and 18.2:1. Not a site token, a deliberate midpoint.
#let ksc-pitch = ksc-graphite.mix(ksc-ink) // #15201D
#let ksc-stone = rgb("#7B7A78") // --color-neutral-500, captions
#let ksc-mist = rgb("#E6E6E5") // --color-neutral-100, hairlines on cream
#let ksc-rose = rgb("#FECDCA") // pale rose, the brief's soft highlight / fill tint

// --- Themes -------------------------------------------------------------------
// A theme is a dictionary: page ground, foreground (type - the logo is recoloured
// to the accent, not to this), the accent (logo, links, small marks), the muted
// tone for secondary lines, and the hairline colour.
// `fg` is body text and `heading` is headings, because on the website they are
// two different colours and using one for both made the page read far harder
// than kampong.social does. The site's <body> is `text-neutral-700
// dark:text-neutral-300` and its headings are `text-neutral-900
// dark:text-neutral-100`; #071411 is the dark-mode GROUND, not type.
// Deliberately darker than the website: body at neutral-800 (12.2:1 against the
// cream, where the site runs 9.4:1) and headings at a midpoint between
// neutral-900 and --color-dark (16.2:1). Print has no
// -webkit-font-smoothing: antialiased thinning the strokes, so it can carry more
// ink than the site does. `muted` is the body colour rather than a lighter grey:
// captions, page numbers and the footer should not read as a different palette.
#let light-theme = (
  bg: ksc-cream, // --color-light  #FDFBF8
  fg: ksc-charcoal, // --color-neutral-800  #2D3532
  heading: ksc-pitch, // midpoint  #15201D
  accent: ksc-red,
  muted: ksc-charcoal, // same as body: captions and footer are not a lighter grey
  rule: ksc-mist,
)
#let dark-theme = (
  bg: ksc-ink, // --color-dark  #071411
  fg: rgb("#CACAC9"), // --color-neutral-200, one step brighter than the site's 300
  heading: ksc-cream, // --color-light  #FDFBF8
  accent: ksc-red,
  muted: rgb("#CACAC9"), // same as body on the dark ground
  rule: rgb("#3E4643"),
)

// --- Embedded brand artwork ---------------------------------------------------
// The canonical marks from ../assets/, single-ink, with `currentColor` swapped
// for the {INK} token. `logo` is the stacked KAMPONG / SOCIAL / CLUB wordmark
// with the palm glyph; `mark` is the square "K" monogram. Both are monochrome by
// design - render them in red on cream, or cream on the dark ground.
#let _svg-logo = "<svg baseProfile='tiny' xmlns='http://www.w3.org/2000/svg' x='0px' y='0px' viewBox='0 0 690 380' overflow='visible' xmlns:xlink='http://www.w3.org/1999/xlink' xml:space='preserve'><g><path fill='{INK}' d='M109.3,71.4l16.1-48.8H99.1l-12.5,42V29.3l-0.7-0.4c-0.9-0.6-1.8-1.1-2.6-1.4l1.8,4.5l-4.6-2.4c-2.7-1.4-5.4-2.2-7.8-2.7L75,31l-4.7-1.4c-0.9-0.3-1.9-0.5-2.8-0.7c1.6,0.7,2.5,1.4,2.6,1.5l5.9,4.7l-4.1-0.7c4.4,3.1,6.6,7.5,6.9,12.8C79.4,60.1,68.2,69.7,68,70l0.6-5.8c0.4-1.5,0.5-2.9,0.6-4.1c-0.1,0.2-0.2,0.3-0.2,0.3l-3.6,5l0.1-6.2c0-2.9-0.4-5.4-0.9-7.3l-2.7,7l-1.1-7.4c-0.3-2.3-1.1-4.2-2.1-5.7v15c5.6,14.4,8.2,28.2,9.9,40.5c0.9,6.8,1.1,14,1,20.6c-1-14.5-3.8-34.9-10.9-55.2c-3.6-10.4-8.4-20.8-14.7-30.3c3,0.3,10.1,1.4,14.7,6.2c2,2.1,3.5,4.9,4.1,8.6c0,0,0.7-1.7,0.9-3.6c0.1-0.8,0.1-1.5-0.1-2.3c0,0,0,0,0,0c0,0,0,0,0,0c0,0,4,4.5,3.9,13.9c0,0,1.6-2.3,2.3-6.5c0,0,2.7,4.7,0.9,11.9c0,0,6.6-7.5,6.3-17.3c-0.2-9.1-7.6-13-13.7-14.5c-0.5-0.1-1-0.2-1.5-0.3c0,0,0.5-0.2,1.5-0.4c1.3-0.3,3.3-0.5,5.8-0.1c0,0-2-1.6-5.9-2.4c-1.3-0.3-2.8-0.4-4.5-0.4c-3,0-6.6,0.6-10.8,2.4c0,0,3.8-3.7,10.8-4.8c1.4-0.2,2.8-0.3,4.4-0.3c2.4,0,5,0.4,7.9,1.3c0,0-1.5-2.7-4.9-3.6c0,0,7.9-0.3,15.4,3.5c0,0-0.9-2.3-3.6-4.1c0,0,4.3,0.5,8.8,3.3c0.1,0.1,0.3,0.2,0.4,0.3c0,0-0.1-0.4-0.4-1c-0.4-0.8-1-2.1-2-3.5c-1.8-2.7-4.8-6-9.2-7.7c-4.5-1.7-8.8-1.8-12.8-0.6c-3.3,1-6.5,3-9.7,5.7c0,0,1.1-3.4,4.7-6.3c0,0-10.6,2.7-14.1,15.1c0,0-1-8.1,8.5-15.6c0,0-2-0.1-4.7,1.2c0,0,3.4-5.1,10.1-7.9c0,0-2.7-0.8-5.4,0.1c0,0,2.1-2.8,7.8-4.5c0,0-6.5-3.4-12.7-0.3C41,5,39.1,12.8,39.4,19.4c0,0-1.6-4.3-1.4-7.7c0,0-1.8,5.3-0.7,9.8c0,0-5.6-7.6-14.4-8.5c0,0,3,1.6,4.3,3.6c0,0-7.5-5.4-15.7-3C3.4,16,1,26.8,1,26.8s3.9-3.1,9.5-4.1c0,0-2.7,2-3.7,5.1c0,0,4.7-4.4,14.9-4c0,0-3.3,1.1-4.9,3c0,0,8.8-3.8,20.7,3.1c0,0-13.3-7.2-25.8,4.5c0,0,2.6-1.4,8.8-1.5c0,0-10.4,1.8-15.3,10.6S2.8,63.7,6.4,68.7c0,0-0.1-7.5,3.4-12.6c0,0-1.1,4.9,1.1,8.1c0,0,0.8-11.4,7.7-18c0,0-1.9,4.5-0.6,7.8c0,0,1.8-16.9,21.2-18.1c0,0-18.8,2.3-18.4,22.1c0,0,2.1-6.4,5.9-9.2c0,0-7,7.9-2.7,18c4.2,10.1,19.5,12.4,24.7,13c0,0-6.5-4.3-9.4-9.5c0,0,4.1,2.8,8.2,3.6c0,0-8-4.5-11-15.2c0,0,2.6,3.6,6.7,5c0,0-11.3-8.3-1.9-25.7l0.5-0.8c2.9,5.9,14.1,32.5,16.7,67.5c0.6,8.2,0.7,16.8,0.2,25.8h10.3h0.9h16.7V84.4l11.4,46.3h29L109.3,71.4z'></path><path fill='{INK}' d='M217,130.7H188l-3.3-16.9h-20.5l-3.3,16.9h-27.7L153,23.3h44.2L217,130.7z M181.4,93.5l-6.1-38.4h-1.2L168,93.5H181.4z'></path><path fill='{INK}' d='M330.6,130.7h-27.3v-56h-1.6l-10.3,56h-28l-9.7-56H252v56h-27l3.7-107.4h39.1l9.7,58.1h1.4l9.4-58.1H327L330.6,130.7z'></path><path fill='{INK}' d='M382.1,95.6h-8v35.1h-29.2V23.3h35.3c12.4,0,21.6,2.8,27.7,8.5c6,5.6,9,14.9,9,27.8c0,12.9-2.8,22.1-8.5,27.7C402.7,92.8,393.9,95.6,382.1,95.6z M385.4,72.2c1-2,1.6-5.5,1.6-10.5c0-5.1-0.6-8.6-1.7-10.7c-1.2-2.1-3.5-3.1-7-3.1h-4.2v27.3h4.9C382.2,75.2,384.4,74.2,385.4,72.2z'></path><path fill='{INK}' d='M432.1,34.3c6.7-8.6,17.6-13,32.8-13c15.2,0,26.1,4.3,32.6,12.8c6.6,8.5,9.8,22.4,9.8,41.6s-3.4,33.4-10.1,42.6c-6.7,9.2-17.6,13.8-32.7,13.8c-15.1,0-25.9-4.6-32.5-13.7c-6.6-9.1-9.9-23.3-9.9-42.4C422.1,56.9,425.5,43,432.1,34.3z M474.9,54.3c-1.7-3.7-5.1-5.6-10.3-5.6c-5.2,0-8.5,1.9-10,5.6c-1.5,3.7-2.3,10.9-2.3,21.4v4.1c0,11.2,0.8,18.5,2.5,22c1.7,3.5,5.1,5.3,10.3,5.3c5.2,0,8.5-1.7,10-5c1.5-3.3,2.3-10.3,2.3-20.7v-5.5C477.4,65.2,476.6,58,474.9,54.3z'></path><path fill='{INK}' d='M598,130.7h-28.5l-22.4-50h-1.4v50h-26.4V23.3h29.4L570,74.8h1.4V23.3H598V130.7z'></path><path fill='{INK}' d='M678.4,55.1c-11.1-3.3-19.3-5-24.4-5c-5.2,0-8.7,2.2-10.6,6.7s-2.9,11.6-2.9,21.4c0,9.7,0.9,16.8,2.7,21.3c1.8,4.4,5.5,6.7,11,6.7c1.6,0,4.8-0.5,9.4-1.4V91.5h-7.8V71.1H689v59.3h-20.5l-2.8-5.5H665c-2,1.6-3.5,2.8-4.7,3.6c-3.2,2.4-7.8,3.6-13.7,3.6c-24.2,0-36.4-18.2-36.4-54.6c0-18.2,3.2-32.1,9.6-41.8c6.4-9.6,17.1-14.4,32.2-14.4c12.1,0,22.4,1.8,31.1,5.5L678.4,55.1z'></path></g><g><path fill='{INK}' d='M64.1,154.3c6-6.2,14.3-9.3,24.8-9.3c10.5,0,21.3,1,32.3,3.1l-3.5,27.7c-11.5-2.7-19.4-4-23.7-4c-5.5,0-8.2,2.2-8.2,6.6c0,1.7,1.3,3.4,3.9,5c2.6,1.6,5.8,3.3,9.5,5.1c3.7,1.8,7.4,3.9,11.1,6.4c3.7,2.5,6.9,6,9.5,10.6c2.6,4.6,3.9,9.9,3.9,15.9c0,11-3,19.4-8.9,25.4c-5.9,6-14.3,9-25.1,9c-10.8,0-21.4-1.7-31.7-5l1.9-25.8c12.4,3.6,21,5.4,25.9,5.4c4.9,0,7.3-2.1,7.3-6.4c0-2.2-1.3-4.2-3.9-6.1c-2.6-1.8-5.8-3.7-9.5-5.5c-3.7-1.8-7.5-4-11.2-6.4c-3.8-2.4-7-5.9-9.6-10.5c-2.6-4.6-3.9-10.1-3.9-16.4C55.1,168.7,58.1,160.5,64.1,154.3z'></path><path fill='{INK}' d='M140.7,157.9c6.7-8.6,17.6-13,32.8-13c15.2,0,26.1,4.3,32.6,12.8c6.6,8.5,9.8,22.4,9.8,41.6s-3.4,33.4-10.1,42.6c-6.7,9.2-17.6,13.8-32.7,13.8c-15.1,0-25.9-4.6-32.5-13.7c-6.6-9.1-9.9-23.3-9.9-42.4C130.7,180.4,134.1,166.5,140.7,157.9z M183.5,177.9c-1.7-3.7-5.1-5.6-10.3-5.6c-5.2,0-8.5,1.9-10,5.6c-1.5,3.7-2.3,10.9-2.3,21.4v4.1c0,11.2,0.8,18.5,2.5,22c1.7,3.5,5.1,5.3,10.3,5.3c5.2,0,8.5-1.7,10-5c1.5-3.3,2.3-10.3,2.3-20.7v-5.5C186.1,188.8,185.2,181.6,183.5,177.9z'></path><path fill='{INK}' d='M254.6,201.4c0,10.2,1,17.5,3,21.8c2,4.3,5.4,6.5,10,6.5c4.6,0,11.2-1.1,19.7-3.3l4.5,24.4c-9.2,3.3-18.4,5-27.7,5c-13.9,0-24-4.6-30.4-13.7c-6.3-9.2-9.5-22.8-9.5-41s3.3-32,9.9-41.7c6.6-9.6,17.4-14.4,32.4-14.4c8.8,0,17.2,1.8,25.2,5.5l-4.7,26.8c-9-3-15.4-4.5-19-4.5c-5.1,0-8.6,2.2-10.6,6.7C255.6,183.8,254.6,191.2,254.6,201.4z'></path><path fill='{INK}' d='M330.4,254h-29.6V146.6h29.6V254z'></path><path fill='{INK}' d='M425.1,254H396l-3.3-16.9h-20.5l-3.3,16.9h-27.7l19.8-107.4h44.2L425.1,254z M389.4,216.8l-6.1-38.4h-1.2l-6.1,38.4H389.4z'></path><path fill='{INK}' d='M489.1,254h-55V146.6h29.7V230h27L489.1,254z'></path></g><g><path fill='{INK}' d='M85.3,324.9c0,10.2,1,17.5,3,21.8c2,4.3,5.4,6.5,10,6.5c4.6,0,11.2-1.1,19.7-3.3l4.5,24.4c-9.2,3.3-18.4,5-27.7,5c-13.9,0-24-4.6-30.4-13.7c-6.3-9.2-9.5-22.8-9.5-41s3.3-32,9.9-41.7c6.6-9.6,17.4-14.4,32.4-14.4c8.8,0,17.2,1.8,25.2,5.5l-4.7,26.8c-9-3-15.4-4.5-19-4.5c-5.1,0-8.6,2.2-10.6,6.7C86.3,307.4,85.3,314.7,85.3,324.9z'></path><path fill='{INK}' d='M184.8,377.6h-55V270.2h29.7v83.3h27L184.8,377.6z'></path><path fill='{INK}' d='M225.6,350.3c1.5,1.7,4,2.6,7.4,2.6c3.4,0,5.8-0.8,7.1-2.5c1.3-1.7,2-4.4,2-8.2v-71.9H272v73.7c0,12.3-3.4,21.3-10.2,27c-6.8,5.6-16.8,8.5-29.9,8.5c-13.2,0-23-2.8-29.4-8.4c-6.4-5.6-9.7-14.5-9.7-26.7v-74h30.4v72.1C223.3,345.9,224,348.5,225.6,350.3z'></path><path fill='{INK}' d='M363.3,346.6c0,20.6-11.1,30.9-33.4,30.9h-42.6V270.2h41.8c11,0,19.2,2.2,24.5,6.7c5.3,4.4,8,11.5,8,21.2c0,9.7-4,17.1-12,22.1v1C358.7,324.8,363.3,333.3,363.3,346.6z M328.7,313.8c1-1.6,1.6-4.5,1.6-8.6c0-4.1-0.6-7-1.7-8.6c-1.1-1.7-3-2.5-5.7-2.5h-5.6v22.1h5.7C325.8,316.2,327.7,315.4,328.7,313.8z M330,356.1c1.2-1.6,1.8-4.8,1.8-9.7c0-4.9-0.6-8.4-1.9-10.4c-1.3-2-3.6-3-7-3h-5.6v25.4h5.6C326.4,358.4,328.8,357.6,330,356.1z'></path></g></svg>"
#let _svg-mark = "<svg baseProfile='tiny' xmlns='http://www.w3.org/2000/svg' x='0px' y='0px' viewBox='34 58 384 396' overflow='visible' xmlns:xlink='http://www.w3.org/1999/xlink' xml:space='preserve'><g><path fill='{INK}' d='M360.8,273.1l47.9-145.2h-78.2l-37.2,124.9v-105l-2.1-1.2c-2.7-1.8-5.4-3.3-7.7-4.2l5.4,13.4l-13.7-7.1c-8-4.2-16.1-6.5-23.2-8l6.8,12.2l-14-4.2c-2.7-0.9-5.7-1.5-8.3-2.1c4.8,2.1,7.4,4.2,7.7,4.5l17.6,14l-12.2-2.1c13.1,9.2,19.6,22.3,20.5,38.1c1.8,38.4-31.5,66.9-32.1,67.8l1.8-17.3c1.2-4.5,1.5-8.6,1.8-12.2c-0.3,0.6-0.6,0.9-0.6,0.9l-10.7,14.9l0.3-18.4c0-8.6-1.2-16.1-2.7-21.7l-8,20.8l-3.3-22c-0.9-6.8-3.3-12.5-6.2-17v44.6c16.7,42.8,24.4,83.9,29.5,120.5c2.7,20.2,3.3,41.6,3,61.3c-3-43.1-11.3-103.8-32.4-164.2c-10.7-30.9-25-61.9-43.7-90.1c8.9,0.9,30,4.2,43.7,18.4c5.9,6.2,10.4,14.6,12.2,25.6c0,0,2.1-5.1,2.7-10.7c0.3-2.4,0.3-4.5-0.3-6.8l0,0l0,0c0,0,11.9,13.4,11.6,41.3c0,0,4.8-6.8,6.8-19.3c0,0,8,14,2.7,35.4c0,0,19.6-22.3,18.7-51.5c-0.6-27.1-22.6-38.7-40.8-43.1c-1.5-0.3-3-0.6-4.5-0.9c0,0,1.5-0.6,4.5-1.2c3.9-0.9,9.8-1.5,17.3-0.3c0,0-5.9-4.8-17.6-7.1c-3.9-0.9-8.3-1.2-13.4-1.2c-8.9,0-19.6,1.8-32.1,7.1c0,0,11.3-11,32.1-14.3c4.2-0.6,8.3-0.9,13.1-0.9c7.1,0,14.9,1.2,23.5,3.9c0,0-4.5-8-14.6-10.7c0,0,23.5-0.9,45.8,10.4c0,0-2.7-6.8-10.7-12.2c0,0,12.8,1.5,26.2,9.8c0.3,0.3,0.9,0.6,1.2,0.9c0,0-0.3-1.2-1.2-3c-1.2-2.4-3-6.2-5.9-10.4c-5.4-8-14.3-17.8-27.4-22.9c-13.4-5.1-26.2-5.4-38.1-1.8c-9.8,3-19.3,8.9-28.9,17c0,0,3.3-10.1,14-18.7c0,0-31.5,8-41.9,44.9c0,0-3-24.1,25.3-46.4c0,0-5.9-0.3-14,3.6c0,0,10.1-15.2,30-23.5c0,0-8-2.4-16.1,0.3c0,0,6.2-8.3,23.2-13.4c0,0-19.3-10.1-37.8-0.9c-18.4,9.2-24.1,32.4-23.2,52.1c0,0-4.8-12.8-4.2-22.9c0,0-5.4,15.8-2.1,29.2c0,0-16.7-22.6-42.8-25.3c0,0,8.9,4.8,12.8,10.7c0,0-22.3-16.1-46.7-8.9c-24.1,7.1-31.2,39.3-31.2,39.3s11.6-9.2,28.3-12.2c0,0-8,5.9-11,15.2c0,0,14-13.1,44.3-11.9c0,0-9.8,3.3-14.6,8.9c0,0,26.2-11.3,61.6,9.2c0,0-39.6-21.4-76.7,13.4c0,0,7.7-4.2,26.2-4.5c0,0-30.9,5.4-45.5,31.5s-7.1,60.1,3.6,75c0,0-0.3-22.3,10.1-37.5c0,0-3.3,14.6,3.3,24.1c0,0,2.4-33.9,22.9-53.5c0,0-5.7,13.4-1.8,23.2c0,0,5.4-50.3,63.1-53.8c0,0-55.9,6.8-54.7,65.7c0,0,6.2-19,17.6-27.4c0,0-20.8,23.5-8,53.5c12.5,30,58,36.9,73.5,38.7c0,0-19.3-12.8-28-28.3c0,0,12.2,8.3,24.4,10.7c0,0-23.8-13.4-32.7-45.2c0,0,7.7,10.7,19.9,14.9c0,0-33.6-24.7-5.7-76.5l1.5-2.4c8.6,17.6,41.9,96.7,49.7,200.8c1.8,24.4,2.1,50,0.6,76.7h30.6h2.7h49.7V311.8l33.9,137.7h86.3L360.8,273.1z'></path></g></svg>"

// --- Asset helpers --------------------------------------------------------------
#let _ink-svg(src, ink, ..sizing) = image(
  bytes(src.replace("{INK}", ink.to-hex())),
  format: "svg",
  ..sizing,
)

// The wordmark, at any width, in any colour. Exposed because decks, covers and
// letterheads all want it and none of them should paste the SVG again.
#let ksc-logo(fill: ksc-red, width: 4cm) = _ink-svg(_svg-logo, fill, width: width)
// The square "K" monogram. The embedded copy's viewBox is trimmed to the glyph's
// real bounding box (measured, not guessed), so this is just an image.
#let ksc-mark(fill: ksc-red, height: 1cm) = _ink-svg(_svg-mark, fill, height: height)

// --- Called-out box -------------------------------------------------------------
// A tinted block with a red spine, for the one thing on the page a reader must
// not skim past. Theme-aware: the tint is drawn from the text colour, so it works
// on cream and on ink without a second set of values.
#let ksc-note(title: none, tint: auto, body) = context {
  let fg = text.fill
  // On the cream ground the fill is the brief's pale rose, well diluted; on the
  // dark ground rose goes muddy, so there it is the text colour at low alpha.
  // Detected from the type colour rather than passed in, so a caller who never
  // thinks about themes still gets the right one.
  let dark-ground = luma(fg).components().first() > 50%
  let fill = if tint != auto { tint } else if dark-ground {
    fg.transparentize(92%)
  } else { ksc-rose.transparentize(62%) }
  block(
    width: 100%,
    above: 1.5em,
    below: 1.5em,
    breakable: false,
    fill: fill,
    stroke: (left: 2.5pt + ksc-red),
    inset: (left: 1.1em, rest: 0.95em),
    radius: (top-right: 2pt, bottom-right: 2pt),
    {
        set par(justify: true, leading: 0.9em)
      if title != none {
        block(below: 0.55em, text(font: "Inter", weight: 700, size: 10pt, fill: fg, title))
      }
      set text(size: 10pt)
      body
    },
  )
}

// --- Screenshots ----------------------------------------------------------------
// Wrap a screenshot in a hairline frame so it reads as an inset object rather
// than bleeding into the page. Theme-aware, like everything else here.
// Square corners on purpose: most app screenshots already carry a rounded card,
// and a rounded frame around a rounded card leaves a sliver of background in
// each corner. Crop inside the card edge and let this be a plain hairline.
#let ksc-shot(path, width: 100%) = context box(
  width: width,
  clip: true,
  stroke: 0.6pt + text.fill.transparentize(82%),
  image(path, width: 100%),
)

// --- Closing page -----------------------------------------------------------------
// A full-bleed back cover: the wordmark at cover size over a contact block, with
// no header, footer or page number. Call it as the last thing in a document.
#let ksc-closing(
  theme: light-theme,
  lines: (),
  note: none,
) = page(header: none, footer: none, margin: (left: 3.2cm, right: 3.2cm, top: 4.2cm, bottom: 3.2cm), {
  set text(font: "Inter", weight: 350, size: 10.5pt, fill: theme.fg)
  set par(justify: false, leading: 0.9em)
  v(1fr)
  align(center, ksc-logo(fill: theme.accent, width: 8cm))
  v(1.4cm)
  // `lines` is a list of (label, value) pairs, set as a two-column grid so the
  // values line up. A centred column of prose read as an afterthought.
  align(center, block(width: 62%, {
    line(length: 100%, stroke: 1.2pt + theme.accent)
    v(1.0em)
    grid(
      columns: (auto, auto),
      column-gutter: 1.4em,
      row-gutter: 0.75em,
      align: (right + horizon, left + horizon),
      ..lines.map(pair => (text(fill: theme.muted, pair.at(0)), pair.at(1))).flatten(),
    )
  }))
  if note != none {
    v(1.1cm)
    align(center, block(width: 76%, text(size: 9pt, fill: theme.muted, note)))
  }
  v(1fr)
})

// --- Document template ------------------------------------------------------------
#let ksc-doc(
  theme: light-theme,
  title: "Document Title",
  subtitle: none,
  // Shown on the cover as "Prepared for <recipient>". `none` omits the line.
  recipient: none,
  author: "Kampong Social Club",
  date: datetime.today(),
  // Footer, column A. The postal address is the operator's registered address,
  // as published in the KSC legal notice.
  org: "Kampong Social Club",
  address: ("1 Phillip Street", "#09-00 Royal One Phillip", "Singapore 048692"),
  // Footer, column B.
  email: "team@kampong.social",
  web: "www.kampong.social",
  social: "@team@kampong.social",
  // Footer, column C. KSC publishes no bank details, so the third column carries
  // the operator and its company number instead.
  legal: ("Operated by Hanso Pte. Ltd.", "UEN 201937629R"),
  // "full" (default) is the 3-column block for anything that leaves KSC;
  // "simple" collapses to one line for internal notes and member handouts.
  footer-style: "full",
  support: "support@kampong.social", // shown in the simple footer
  confidentiality: "Member information", // classification note in the simple footer
  // A short document does not need a table of contents.
  contents: true,
  body,
) = {
  let fg = theme.fg
  let simple = footer-style == "simple"

  // Running header: the wordmark top-left, a hairline beneath it. The brief puts
  // the logo top-left on every surface; this is the document reading of that.
  let header = context {
    if counter(page).get().first() <= 1 { return }
    align(center, ksc-mark(fill: theme.accent, height: 1.3cm))
  }

  // Footer, both styles, share the page counter line.
  let rule-row = context {
    let last = counter(page).final().first()
    align(center, text(fill: theme.muted)[Page #counter(page).display() of #last])
  }

  // Full footer: company + address | contact | operator. Wider than the text
  // column, hence the negative pad.
  let footer-full = pad(x: -1.4cm, {
    set text(font: "Inter", weight: 400, size: 8.5pt, fill: theme.muted)
    set par(leading: 0.55em, justify: false)
    rule-row
    v(6pt)
    let col-a = (text(weight: 600, fill: theme.heading, org),) + address
    let col-b = (email, web, social).filter(x => x != none and x != "")
    let col-c = legal
    let rows = calc.max(col-a.len(), col-b.len(), col-c.len())
    grid(
      columns: (1.1fr, 1fr, 1fr),
      column-gutter: 1.2em,
      row-gutter: 0.7em,
      align: left + top,
      ..range(rows)
        .map(i => (col-a.at(i, default: []), col-b.at(i, default: []), col-c.at(i, default: [])))
        .flatten(),
    )
  })

  // Simple footer: the rule row, then one line spread across the width.
  // A centred page number on every page but the last, which carries no footer at
  // all. The contact line lives in the body of the closing chapter instead of
  // being repeated under every page.
  let footer-simple = context pad(x: -1.4cm, {
    if counter(page).get().first() == counter(page).final().first() { return }
    set text(font: "Inter", weight: 400, size: 8.5pt, fill: theme.muted)
    set par(leading: 0.55em, justify: false)
    rule-row
    return
    v(6pt)
    let items = (
      support,
      web,
      if confidentiality != none { text(style: "italic", confidentiality) },
      social,
    ).filter(x => x != none and x != "")
    grid(
      columns: items.len(),
      align: horizon,
      column-gutter: 1fr,
      ..items,
    )
  })

  set document(title: title, author: author)
  // Hurenkinder und Schusterjungen. Typst exposes these as layout costs rather
  // than hard rules: raising widow/orphan makes the engine work much harder to
  // avoid stranding the tail of a paragraph on a page of its own, and `runt`
  // discourages a last line with almost nothing on it.
  set text(
    font: "Inter",
    weight: 350,
    size: 10.5pt,
    fill: fg,
    lang: "en",
    hyphenate: true,
    costs: (widow: 800%, orphan: 800%, runt: 300%, hyphenation: 100%),
  )
  // Justified, hyphenated, at the website's line-height. Measured: leading 0.72em
  // gave 1.45; kampong.social sets its prose at 1.75, which is leading 1.0em here.
  set par(justify: true, leading: 1.0em, spacing: 1.45em)

  set page(
    paper: "a4",
    fill: theme.bg,
    margin: (
      left: 3.2cm,
      right: 3.2cm,
      // The header sits at `top - header-ascent` from the paper edge, so the top
      // margin is what buys the wordmark its inset. At 3.6cm it sat 0.7cm off the
      // edge and read as though it had slipped off the page.
      top: 4.2cm,
      bottom: if simple { 3.2cm } else { 4.4cm },
    ),
    header: header,
    header-ascent: 1.5cm,
    footer: if simple { footer-simple } else { footer-full },
    footer-descent: if simple { 1.1cm } else { 1.4cm },
  )

  // Links carry the accent - the web app's --tw-prose-links is the brand red.
  show link: set text(fill: theme.accent)

  // Headings: chapter I. II. III. (uppercase, new page) / section a. b. c. / 1. 2. 3.
  set heading(numbering: (..n) => {
    let pattern = ("I.", "a.", "1.").at(calc.min(n.pos().len(), 3) - 1)
    numbering(pattern, n.pos().last())
  })
  show heading: set block(sticky: true) // never strand a heading at a page foot
  show heading: it => {
    let lvl = calc.min(it.level, 3)
    // Passion One is the display face; level 3 is a bold Inter subhead, which is
    // what the deck does and what reads at that size. Fill is pinned to ink so a
    // body-level `set text` cannot turn a heading red.
    let blk = {
      set text(
        font: ("Passion One", "Passion One", "Inter").at(lvl - 1),
        fill: theme.heading,
        weight: (400, 400, 700).at(lvl - 1),
        size: (30pt, 19pt, 11.5pt).at(lvl - 1),
      )
      set par(leading: 0.3em, justify: false)
      block(
        above: if lvl == 1 { 0pt } else { 1.5em },
        below: (0.8em, 0.6em, 0.45em).at(lvl - 1),
        {
          if it.numbering != none { box[#counter(heading).display(it.numbering)#h(0.4em)] }
          if lvl == 1 { upper(it.body) } else { it.body }
        },
      )
    }
    if lvl == 1 {
      pagebreak(weak: true)
      blk
    } else { blk }
  }

  // Standfirst under a chapter title: muted italic, no rules, no red - the brief
  // keeps red for the logo and small marks.
  show quote.where(block: true): it => block(
    width: 100%,
    above: 0.4em,
    below: 1.5em,
    {
      set text(style: "italic", fill: theme.muted, size: 12pt)
      set par(leading: 0.72em, justify: false)
      it.body
      if it.attribution != none {
        v(0.3em)
        text(size: 10pt)[#sym.dash.en #it.attribution]
      }
    },
  )

  set list(marker: text(fill: theme.accent, size: 0.85em)[#sym.bullet], indent: 2pt, body-indent: 0.7em, spacing: 0.85em)
  set enum(indent: 2pt, body-indent: 0.7em, spacing: 0.85em)

  // Inline code / hostnames: this template gets used for setup instructions, and
  // a raw span has to be legible without turning into a second brand colour.
  show raw.where(block: false): it => box(
    fill: fg.transparentize(94%),
    inset: (x: 0.28em, y: 0.15em),
    outset: (y: 0.2em),
    radius: 2pt,
    text(size: 0.92em, fill: fg, it),
  )

  // Figures: centred, numbered caption with the label in the accent.
  show figure: set block(above: 1.6em, below: 1.6em)
  set figure(gap: 0.85em)
  show figure.caption: it => {
    set text(font: "Inter", fill: theme.muted, size: 9pt)
    set par(leading: 0.5em, justify: false)
    if it.numbering != none {
      text(weight: 700, fill: theme.accent)[#it.supplement #it.counter.display(it.numbering)]
      [ #sym.dash.en ]
    }
    it.body
  }

  // Tables: an accent rule under the header row, faint hairlines between body
  // rows, no verticals.
  set table(
    stroke: (x, y) => (bottom: if y == 0 { 1pt + theme.accent } else { 0.5pt + fg.transparentize(80%) }),
    inset: (x: 0.7em, y: 0.6em),
    align: left + horizon,
  )
  show table.cell.where(y: 0): set text(weight: 700, fill: theme.heading)
  // Justification stretches short cells into gap-toothed lines; tables read as
  // columns, not as prose.
  show table: set par(justify: false)

  // ------- Cover: full bleed, no header or footer -------
  page(header: none, footer: none, margin: (left: 3.2cm, right: 3.2cm, top: 4.2cm, bottom: 3.2cm), {
    ksc-logo(fill: theme.accent, width: 5.4cm)
    v(1fr)
    block(width: 100%, {
      set par(justify: false, leading: 0.24em)
      text(font: "Passion One", weight: 400, size: 88pt, hyphenate: false, fill: theme.heading, upper(title))
    })
    if subtitle != none {
      v(0.7em)
      block(width: 88%, {
        set par(justify: false, leading: 0.75em)
        text(size: 15pt, fill: theme.muted, subtitle)
      })
    }
    v(1fr)
    line(length: 100%, stroke: 1.2pt + theme.accent)
    v(0.7em)
    block({
      set par(justify: false, leading: 0.7em)
      set text(size: 10.5pt, fill: theme.muted)
      if recipient != none [Prepared for #text(fill: theme.heading, weight: 600, recipient) \ ]
      [#author #h(0.35em) #sym.dot.c #h(0.35em) #date.display("[day] [month repr:long] [year]")]
    })
  })

  // ---------------- Contents ----------------
  if contents {
    show outline.entry: set text(weight: 400)
    show outline.entry: set block(above: 1.1em)
    show outline.entry.where(level: 1): set block(above: 1.6em)
    set outline.entry(fill: none)
    outline(
      title: block(below: 0.9cm, text(font: "Passion One", weight: 400, size: 30pt, fill: theme.heading, upper[Contents])),
      depth: 2,
      indent: 1.2em,
    )
  }

  // ---------------- Body ----------------
  body
}
