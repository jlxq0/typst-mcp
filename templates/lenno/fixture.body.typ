= Operating context

#quote(block: true)[The goal is a system that stays understandable as the number of agents grows.]

Lenno gives teams one place to define, run and observe coordinated agent work. This review
focuses on reliability, ownership and recovery. Each recommendation has a named operator and
a result that can be verified without private context.

== Priorities

- Make workflow ownership visible before a run begins.
- Keep execution state inspectable across every handoff.
- Recover cleanly when one agent or external dependency fails.

#lenno-note(title: "Decision", theme: dark-theme)[Adopt one reviewed workflow for production
changes and require an evidence envelope at each approval boundary.]

= Delivery plan

== First month

#figure(
  table(
    columns: (1fr, 2fr, 1fr),
    stroke: 0.6pt + lenno-dark-edge,
    inset: 8pt,
    table.header([Workstream], [Outcome], [Owner]),
    [Workflow], [One production path with explicit approval boundaries], [Platform],
    [Evidence], [A durable run envelope for every handoff], [Operations],
    [Recovery], [A rehearsed fallback for provider and worker failure], [Reliability],
  ),
  caption: [First-month commitments.],
)

== Measures

#grid(
  columns: (1fr, 1fr, 1fr),
  gutter: 8pt,
  lenno-stat("12", "workflows reviewed", theme: dark-theme),
  lenno-stat("4", "teams onboarded", theme: dark-theme),
  lenno-stat("99.9%", "availability target", theme: dark-theme),
)

We will track recovery time, unresolved handoffs and the share of runs with complete
evidence. The useful measure is whether an operator can understand and resume work without
asking the previous operator to reconstruct the run.
