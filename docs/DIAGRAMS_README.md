# ProximaDB Diagram Generation Guide

## Overview

ProximaDB documentation uses PlantUML and Mermaid diagrams that must be converted to PNG images for GitHub rendering.

## Required Tools

### PlantUML
```bash
# Ubuntu/Debian
sudo apt-get install plantuml

# macOS
brew install plantuml

# Or download JAR directly
wget https://github.com/plantuml/plantuml/releases/download/v1.2024.0/plantuml-1.2024.0.jar
```

### Mermaid CLI
```bash
npm install -g @mermaid-js/mermaid-cli
```

## Generating Diagrams

### Quick Generation (All Diagrams)
```bash
cd docs
./generate_diagrams.sh
```

### Manual Generation

#### PlantUML
```bash
cd docs
# Generate all PlantUML diagrams
plantuml -tpng -o diagrams/images diagrams/plantuml/*.puml

# Or using JAR file
java -jar plantuml.jar -tpng -o diagrams/images diagrams/plantuml/*.puml
```

#### Mermaid
```bash
cd docs
# Generate all Mermaid diagrams
for f in diagrams/*.mmd; do
  mmdc -i "$f" -o "diagrams/images/$(basename "$f" .mmd).png" -t dark -b transparent
done
```

## Key Diagrams for Documentation

The following diagrams are referenced in the main documentation and must be generated:

### Architecture Documentation (`architecture.adoc`)
- `proximadb-component.png` - System component overview
- `proximadb-class-storage.png` - Storage layer architecture
- `unified_memtable_architecture.png` - Memtable design
- `unified_quantization_system.png` - Quantization architecture

### Technical Reference (`technical_reference.adoc`)
- `proximadb-sequence-insert.png` - Insert operation flow
- `proximadb-sequence-search.png` - Search operation flow
- `proximadb-activity-flush.png` - WAL flush process
- `proximadb-state-vector.png` - Vector lifecycle

### API Documentation (`api/unified_rest_api.adoc`)
- `search-optimization-flow.png` - Search optimization
- `quantization-components.png` - Quantization components

## Verification

After generating diagrams, verify they exist:
```bash
ls -la docs/diagrams/images/*.png | wc -l
# Should show 56 files
```

## CI/CD Integration

For automated builds, add to your CI pipeline:
```yaml
- name: Generate Diagrams
  run: |
    sudo apt-get install -y plantuml
    npm install -g @mermaid-js/mermaid-cli
    cd docs && ./generate_diagrams.sh
```

## Troubleshooting

### PlantUML Issues
- Ensure Java is installed: `java -version`
- Check PlantUML version: `plantuml -version`
- For syntax errors, test individual files: `plantuml -tpng -syntax diagrams/plantuml/file.puml`

### Mermaid Issues
- Check Node.js version: `node --version` (requires v14+)
- For rendering issues, try: `mmdc -p puppeteer-config.json`
- Use `-t default` for light theme if dark theme has issues

## Adding New Diagrams

1. Create diagram source file:
   - PlantUML: `docs/diagrams/plantuml/proximadb-new-feature.puml`
   - Mermaid: `docs/diagrams/new-feature.mmd`

2. Generate PNG: Run generation commands above

3. Reference in AsciiDoc:
   ```asciidoc
   image::proximadb-new-feature.png[Description,width=100%]
   ```

4. Commit both source and generated PNG files