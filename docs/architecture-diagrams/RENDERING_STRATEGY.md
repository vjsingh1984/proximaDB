This document defines how the ProximaDB architecture atlas is rendered so it is **natively
visible on GitHub** with zero tooling, while keeping full-fidelity UML source available for
non-GitHub publishing.
| Format | GitHub `.md` render | Standalone file | Best for |
|---|---|---|---|
| **Mermaid** (in `.md`) | native | raw text | **PRIMARY - everything Mermaid can express** |
| **PlantUML** `.puml` | needs Kroki | raw text | Full-fidelity UML source (class/sequence/deployment) |
| **PNG / SVG** (exported) | via `![](...)` | yes | Embedding in non-GitHub docs, PDFs, slides |
| **ASCII art** | native (code block) | yes | Simple ideas, inline in prose, email-safe |
1. **Mermaid-first.** When Mermaid and PlantUML can both express a diagram, prefer **Mermaid**
   so it renders natively on GitHub.
2. **PlantUML kept as source-of-truth** for the richer UML views (deployment, class, sequence)
   that benefit from PlantUML's layout - then exported to PNG/SVG for embedding.
3. **Kroki fallback.** PlantUML renders inline on GitHub by pointing the image at the public
   Kroki server: `https://kroki.io/plantuml/svg/<base64-puml>`. Use when a diagram must stay
   PlantUML but still render on GitHub.
4. **ASCII art for the simplest ideas** (1-box ownership, inline in prose) - native everywhere,
   no tooling, copy-paste-safe.
1. Can a reader see it on GitHub with **zero install**? -> Mermaid in `.md` wins.
2. Is it a class/sequence/deployment diagram needing PlantUML's layout? -> `.puml` + **exported
   PNG** committed next to it, embedded via `![](./x.png)`.
3. Is it a one-off simple idea? -> ASCII art inline.
```bash
bash scripts/diagrams/render_atlas.sh
bash scripts/diagrams/render_atlas.sh --kroki
```
See `atlas.md` for the **all-Mermaid, natively-rendered single-page view** of the system.
