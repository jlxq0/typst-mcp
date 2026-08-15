= Scope

#quote(block: true)[A standfirst introduces the chapter — the italic accent line that sits under a chapter title.]

This document exercises the template end to end: cover, table of contents, branded
headings, body text, lists, tables and a bar chart.

== What was reviewed

- Ingress and TLS termination at the edge
- Authentication on every public endpoint
- Secret handling and rotation

== Findings at a glance

#figure(
  table(
    columns: 3,
    table.header([Area], [Finding], [Severity]),
    [Ingress], [TLS terminates at the edge as designed], [None],
    [Auth], [All public routes require a bearer token], [None],
    [Secrets], [Rotation is manual], [Low],
  ),
  caption: [Summary of review findings.],
)

= Detail

== Coverage by area

#figure(
  hanso-barchart((
    ("Ingress", 100, "100%", sunflower-yellow),
    ("Auth", 92, "92%", gerbera-red),
    ("Secrets", 68, "68%", crimson-red),
  )),
  caption: [Review coverage by area.],
)

== Recommendation

Automate secret rotation in the next cycle. Everything else is operating as intended.
