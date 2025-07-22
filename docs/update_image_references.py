#!/usr/bin/env python3
"""
Update AsciiDoc files to use generated PNG images instead of inline diagrams.
Lists PlantUML and Mermaid files that need PNG generation.
"""

import os
import re
from pathlib import Path

def find_diagram_files(docs_dir):
    """Find all PlantUML and Mermaid diagram files."""
    plantuml_files = []
    mermaid_files = []
    
    # Find PlantUML files
    plantuml_dir = os.path.join(docs_dir, "diagrams", "plantuml")
    if os.path.exists(plantuml_dir):
        for file in os.listdir(plantuml_dir):
            if file.endswith(".puml"):
                plantuml_files.append(file)
    
    # Find Mermaid files
    diagrams_dir = os.path.join(docs_dir, "diagrams")
    if os.path.exists(diagrams_dir):
        for file in os.listdir(diagrams_dir):
            if file.endswith(".mmd"):
                mermaid_files.append(file)
    
    return sorted(plantuml_files), sorted(mermaid_files)

def get_expected_images(plantuml_files, mermaid_files):
    """Get list of expected PNG files."""
    expected_images = []
    
    # PlantUML images
    for puml in plantuml_files:
        png_name = puml.replace(".puml", ".png")
        expected_images.append(png_name)
    
    # Mermaid images
    for mmd in mermaid_files:
        png_name = mmd.replace(".mmd", ".png")
        expected_images.append(png_name)
    
    return sorted(expected_images)

def check_existing_images(docs_dir, expected_images):
    """Check which images already exist."""
    images_dir = os.path.join(docs_dir, "diagrams", "images")
    existing_images = []
    missing_images = []
    
    for img in expected_images:
        img_path = os.path.join(images_dir, img)
        if os.path.exists(img_path):
            existing_images.append(img)
        else:
            missing_images.append(img)
    
    return existing_images, missing_images

def main():
    docs_dir = "/home/vsingh/code/proximaDB/docs"
    
    # Find diagram files
    plantuml_files, mermaid_files = find_diagram_files(docs_dir)
    
    print("=== PlantUML Files Found ===")
    print(f"Total: {len(plantuml_files)}")
    for i, file in enumerate(plantuml_files[:10], 1):
        print(f"{i}. {file}")
    if len(plantuml_files) > 10:
        print(f"... and {len(plantuml_files) - 10} more")
    
    print("\n=== Mermaid Files Found ===")
    print(f"Total: {len(mermaid_files)}")
    for i, file in enumerate(mermaid_files, 1):
        print(f"{i}. {file}")
    
    # Get expected images
    expected_images = get_expected_images(plantuml_files, mermaid_files)
    
    # Check existing images
    existing_images, missing_images = check_existing_images(docs_dir, expected_images)
    
    print(f"\n=== Image Status ===")
    print(f"Expected images: {len(expected_images)}")
    print(f"Existing images: {len(existing_images)}")
    print(f"Missing images: {len(missing_images)}")
    
    if missing_images:
        print("\n=== Missing Images (need generation) ===")
        for img in missing_images[:20]:
            source = img.replace(".png", ".puml") if "proximadb-" in img else img.replace(".png", ".mmd")
            print(f"- {img} (from {source})")
        if len(missing_images) > 20:
            print(f"... and {len(missing_images) - 20} more")
    
    print("\n=== Generation Commands ===")
    print("To generate PlantUML images:")
    print("  plantuml -tpng -o ../images diagrams/plantuml/*.puml")
    print("\nTo generate Mermaid images:")
    print("  for f in diagrams/*.mmd; do")
    print("    mmdc -i \"$f\" -o \"diagrams/images/$(basename \"$f\" .mmd).png\"")
    print("  done")
    
    print("\n=== Key Diagrams for Architecture Docs ===")
    key_diagrams = [
        "proximadb-component.png",
        "proximadb-class-services.png", 
        "proximadb-class-storage.png",
        "proximadb-sequence-insert.png",
        "proximadb-sequence-search.png",
        "proximadb-activity-flush.png",
        "proximadb-deployment.png",
        "proximadb-deployment-k8s.png",
        "proximadb-state-vector.png",
        "proximadb-data-flow.png"
    ]
    
    for diagram in key_diagrams:
        status = "✓" if diagram in existing_images else "✗"
        print(f"{status} {diagram}")

if __name__ == "__main__":
    main()