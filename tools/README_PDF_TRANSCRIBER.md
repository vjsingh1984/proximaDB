# PDF Transcriber Tool

A comprehensive Python tool for extracting text from PDF documents, supporting both text-based PDFs and scanned image PDFs through OCR.

## Features

- **Multi-format Support**: Handles both text-based and image-based (scanned) PDFs
- **OCR Capability**: Automatic OCR for scanned documents using Tesseract
- **Safe Operation**: Read-only mode ensures original PDFs are never modified
- **Integrity Verification**: MD5 hash verification before and after processing
- **Multiple Output Formats**: Text, JSON, or Markdown output
- **Multi-page Support**: Processes entire documents efficiently
- **Error Handling**: Robust error handling with detailed reporting
- **Progress Tracking**: Real-time progress updates for large documents

## Installation

### Prerequisites

1. **Python 3.7+**
2. **Tesseract OCR** (for image-based PDFs)

#### Install Tesseract

**macOS:**
```bash
brew install tesseract
brew install tesseract-lang  # For additional languages
```

**Ubuntu/Debian:**
```bash
sudo apt-get update
sudo apt-get install tesseract-ocr
sudo apt-get install tesseract-ocr-fra  # For French, example
```

**Windows:**
Download installer from: https://github.com/UB-Mannheim/tesseract/wiki

### Python Dependencies

```bash
pip install PyPDF2 pdf2image pytesseract Pillow PyMuPDF
```

Or use the script's auto-installation feature - it will install missing packages automatically.

## Usage

### Basic Usage

```bash
# Simple text extraction (auto-detects text vs image PDFs)
python pdf_transcriber.py document.pdf

# Save output to file
python pdf_transcriber.py document.pdf -o output.txt

# Extract with specific method
python pdf_transcriber.py document.pdf --method text  # Text-only extraction
python pdf_transcriber.py document.pdf --method ocr   # Force OCR
```

### Safe Transcriber (Recommended)

The safe transcriber adds extra protection to ensure PDFs are never modified:

```bash
# Safe transcription (works on a copy)
python safe_pdf_transcriber.py document.pdf -o output.txt

# Verify file integrity only
python safe_pdf_transcriber.py document.pdf --verify-only

# Work directly on original (not recommended)
python safe_pdf_transcriber.py document.pdf --no-copy
```

### Output Formats

```bash
# Text format (default)
python pdf_transcriber.py document.pdf -f text

# JSON format (includes metadata)
python pdf_transcriber.py document.pdf -f json -o output.json

# Markdown format (good for documentation)
python pdf_transcriber.py document.pdf -f markdown -o output.md
```

### OCR Options

```bash
# Specify OCR language
python pdf_transcriber.py document.pdf --ocr-lang fra  # French
python pdf_transcriber.py document.pdf --ocr-lang deu  # German
python pdf_transcriber.py document.pdf --ocr-lang "eng+spa"  # Multiple languages

# Adjust OCR quality (higher DPI = better quality but slower)
python pdf_transcriber.py document.pdf --dpi 150  # Faster, lower quality
python pdf_transcriber.py document.pdf --dpi 300  # Default
python pdf_transcriber.py document.pdf --dpi 600  # Slower, higher quality

# Disable OCR entirely
python pdf_transcriber.py document.pdf --no-ocr
```

## Python API Usage

```python
from pdf_transcriber import PDFTranscriber

# Create transcriber
transcriber = PDFTranscriber(
    use_ocr=True,
    ocr_language='eng',
    dpi=300,
    output_format='text'
)

# Transcribe PDF
text = transcriber.transcribe(
    pdf_path='document.pdf',
    output_path='output.txt',  # Optional
    method='auto'  # 'auto', 'text', or 'ocr'
)

# Access statistics
print(f"Total pages: {transcriber.stats['total_pages']}")
print(f"Text pages: {transcriber.stats['text_pages']}")
print(f"OCR pages: {transcriber.stats['ocr_pages']}")
```

## Safe API Usage

```python
from safe_pdf_transcriber import SafePDFTranscriber

# Create safe transcriber
safe_transcriber = SafePDFTranscriber()

# Transcribe with integrity verification
text = safe_transcriber.transcribe_safe(
    pdf_path='document.pdf',
    output_path='output.txt',
    use_copy=True,  # Work on a copy
    use_ocr=True,
    output_format='markdown'
)
```

## Output Examples

### Text Format
```
================================================================================
PDF Transcription: sample.pdf
Date: 2025-01-25 10:30:00
Total Pages: 3
================================================================================

========================================
PAGE 1
========================================
This is the content of page 1...

========================================
PAGE 2
========================================
This is the content of page 2...
```

### JSON Format
```json
{
  "source": "sample.pdf",
  "timestamp": "2025-01-25T10:30:00",
  "stats": {
    "total_pages": 3,
    "text_pages": 2,
    "ocr_pages": 1,
    "errors": []
  },
  "pages": {
    "1": "Content of page 1...",
    "2": "Content of page 2...",
    "3": "Content of page 3..."
  }
}
```

### Markdown Format
```markdown
# PDF Transcription

**Source:** `sample.pdf`
**Date:** 2025-01-25 10:30:00
**Total Pages:** 3

---

## Page 1

Content of page 1...

---

## Page 2

Content of page 2...
```

## Integrity Verification

The safe transcriber provides integrity verification:

```
📄 Original file: /path/to/document.pdf
🔒 File hash: a1b2c3d4e5f6...
📊 File size: 1,234,567 bytes
🔄 Working on copy: /tmp/pdf_transcriber_xyz/document.pdf

📝 Starting transcription...

✅ Integrity Check:
  Original hash: a1b2c3d4e5f6...
  Final hash:    a1b2c3d4e5f6...
  Hash match:    ✅ YES
  Size unchanged: ✅ YES
  Modified time:  ✅ UNCHANGED
```

## Troubleshooting

### Common Issues

1. **"Tesseract not found"**
   - Install Tesseract OCR (see Prerequisites)
   - On Windows, add Tesseract to PATH

2. **"No text extracted"**
   - PDF might be scanned/image-based
   - Try forcing OCR: `--method ocr`

3. **"OCR quality poor"**
   - Increase DPI: `--dpi 600`
   - Ensure correct language: `--ocr-lang [language]`

4. **"Memory error with large PDFs"**
   - Process in batches
   - Reduce DPI for OCR
   - Use text extraction instead of OCR when possible

5. **"Permission denied"**
   - Check file permissions
   - Use safe transcriber with `--no-copy` flag cautiously

## Language Codes for OCR

Common Tesseract language codes:
- `eng` - English
- `fra` - French
- `deu` - German
- `spa` - Spanish
- `ita` - Italian
- `por` - Portuguese
- `rus` - Russian
- `jpn` - Japanese
- `chi_sim` - Chinese Simplified
- `chi_tra` - Chinese Traditional

Use `+` to combine: `--ocr-lang "eng+fra"`

## Performance Tips

1. **Text-based PDFs**: Use `--method text` for fastest extraction
2. **Large documents**: Lower DPI for OCR to reduce processing time
3. **Batch processing**: Process multiple PDFs in parallel using Python's multiprocessing
4. **Memory management**: The tool automatically manages memory for large files

## Security Features

- **Read-only mode**: Files are opened in binary read mode only
- **No write operations**: The transcriber never attempts to write to the source PDF
- **Integrity verification**: MD5 hash verification ensures file remains unchanged
- **Temporary copies**: Safe transcriber works on copies by default
- **Permission preservation**: Original file permissions are maintained

## License

This tool is part of the ProximaDB project and follows the same Apache 2.0 license.

## Contributing

Contributions are welcome! Please ensure:
1. Code follows Python PEP 8 style guidelines
2. All functions have docstrings
3. Error handling is comprehensive
4. File safety is maintained

## Support

For issues or questions, please file an issue in the ProximaDB repository.