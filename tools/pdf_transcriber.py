#!/usr/bin/env python3
"""
PDF Transcriber - Extract text from PDF documents including scanned images
Supports both text-based PDFs and image-based PDFs (using OCR)
"""

import os
import sys
import argparse
from pathlib import Path
from typing import List, Optional, Dict, Any
import json
from datetime import datetime

# Core PDF libraries
try:
    import PyPDF2
except ImportError:
    print("Installing PyPDF2...")
    os.system(f"{sys.executable} -m pip install PyPDF2")
    import PyPDF2

try:
    from pdf2image import convert_from_path
except ImportError:
    print("Installing pdf2image...")
    os.system(f"{sys.executable} -m pip install pdf2image")
    from pdf2image import convert_from_path

try:
    import pytesseract
except ImportError:
    print("Installing pytesseract...")
    os.system(f"{sys.executable} -m pip install pytesseract")
    import pytesseract

try:
    from PIL import Image
except ImportError:
    print("Installing Pillow...")
    os.system(f"{sys.executable} -m pip install Pillow")
    from PIL import Image

try:
    import fitz  # PyMuPDF
except ImportError:
    print("Installing PyMuPDF...")
    os.system(f"{sys.executable} -m pip install PyMuPDF")
    import fitz


class PDFTranscriber:
    """
    A comprehensive PDF transcriber that handles both text and image-based PDFs
    """

    def __init__(self,
                 use_ocr: bool = True,
                 ocr_language: str = 'eng',
                 dpi: int = 300,
                 output_format: str = 'text'):
        """
        Initialize PDF transcriber

        Args:
            use_ocr: Whether to use OCR for image-based pages
            ocr_language: Language for OCR (default: English)
            dpi: DPI for image conversion (higher = better quality but slower)
            output_format: Output format ('text', 'json', 'markdown')
        """
        self.use_ocr = use_ocr
        self.ocr_language = ocr_language
        self.dpi = dpi
        self.output_format = output_format
        self.stats = {
            'total_pages': 0,
            'text_pages': 0,
            'ocr_pages': 0,
            'errors': []
        }

    def extract_text_pypdf2(self, pdf_path: str) -> Dict[int, str]:
        """
        Extract text using PyPDF2 (for text-based PDFs)

        Args:
            pdf_path: Path to PDF file

        Returns:
            Dictionary mapping page numbers to extracted text
        """
        text_by_page = {}

        try:
            # Open in read-only binary mode to avoid any modifications
            with open(pdf_path, 'rb') as file:
                pdf_reader = PyPDF2.PdfReader(file)
                num_pages = len(pdf_reader.pages)

                for page_num in range(num_pages):
                    try:
                        page = pdf_reader.pages[page_num]
                        text = page.extract_text()

                        if text and text.strip():
                            text_by_page[page_num + 1] = text
                            self.stats['text_pages'] += 1
                        else:
                            text_by_page[page_num + 1] = ""
                    except Exception as e:
                        print(f"Error extracting text from page {page_num + 1}: {e}")
                        text_by_page[page_num + 1] = ""
                        self.stats['errors'].append(f"Page {page_num + 1}: {str(e)}")

        except Exception as e:
            print(f"Error reading PDF with PyPDF2: {e}")
            self.stats['errors'].append(f"PyPDF2 error: {str(e)}")

        return text_by_page

    def extract_text_pymupdf(self, pdf_path: str) -> Dict[int, str]:
        """
        Extract text using PyMuPDF (more robust for complex PDFs)
        Opens PDF in read-only mode to prevent any modifications.

        Args:
            pdf_path: Path to PDF file

        Returns:
            Dictionary mapping page numbers to extracted text
        """
        text_by_page = {}

        try:
            # Open PDF in read-only mode (no write permissions)
            # PyMuPDF doesn't modify files by default, but we're explicit
            pdf_document = fitz.open(pdf_path, filetype="pdf")

            for page_num in range(len(pdf_document)):
                try:
                    page = pdf_document[page_num]
                    text = page.get_text()

                    if text and text.strip():
                        text_by_page[page_num + 1] = text
                        self.stats['text_pages'] += 1
                    else:
                        text_by_page[page_num + 1] = ""
                except Exception as e:
                    print(f"Error extracting text from page {page_num + 1}: {e}")
                    text_by_page[page_num + 1] = ""
                    self.stats['errors'].append(f"Page {page_num + 1}: {str(e)}")

            pdf_document.close()

        except Exception as e:
            print(f"Error reading PDF with PyMuPDF: {e}")
            self.stats['errors'].append(f"PyMuPDF error: {str(e)}")

        return text_by_page

    def extract_text_with_ocr(self, pdf_path: str) -> Dict[int, str]:
        """
        Extract text from PDF using OCR (for scanned/image PDFs)

        Args:
            pdf_path: Path to PDF file

        Returns:
            Dictionary mapping page numbers to extracted text
        """
        text_by_page = {}

        try:
            # Convert PDF to images
            print(f"Converting PDF to images (DPI: {self.dpi})...")
            images = convert_from_path(pdf_path, dpi=self.dpi)

            for i, image in enumerate(images, start=1):
                print(f"Processing page {i}/{len(images)} with OCR...")
                try:
                    # Perform OCR on the image
                    text = pytesseract.image_to_string(
                        image,
                        lang=self.ocr_language
                    )

                    if text and text.strip():
                        text_by_page[i] = text
                        self.stats['ocr_pages'] += 1
                    else:
                        text_by_page[i] = ""

                except Exception as e:
                    print(f"OCR error on page {i}: {e}")
                    text_by_page[i] = ""
                    self.stats['errors'].append(f"OCR page {i}: {str(e)}")

        except Exception as e:
            print(f"Error during OCR processing: {e}")
            self.stats['errors'].append(f"OCR error: {str(e)}")

        return text_by_page

    def transcribe(self, pdf_path: str,
                   output_path: Optional[str] = None,
                   method: str = 'auto') -> str:
        """
        Main transcription method

        Args:
            pdf_path: Path to input PDF file
            output_path: Optional path to save output
            method: Extraction method ('text', 'ocr', 'auto')

        Returns:
            Extracted text as string
        """
        if not os.path.exists(pdf_path):
            raise FileNotFoundError(f"PDF file not found: {pdf_path}")

        print(f"Starting transcription of: {pdf_path}")
        print(f"Method: {method}")

        # Reset stats
        self.stats = {
            'total_pages': 0,
            'text_pages': 0,
            'ocr_pages': 0,
            'errors': [],
            'start_time': datetime.now().isoformat()
        }

        text_by_page = {}

        if method == 'auto':
            # Try text extraction first
            print("Attempting text extraction...")
            text_by_page = self.extract_text_pymupdf(pdf_path)

            # Check if we got meaningful text
            total_chars = sum(len(text) for text in text_by_page.values())

            if total_chars < 100 and self.use_ocr:
                # Very little text extracted, probably scanned PDF
                print("Minimal text found, switching to OCR...")
                text_by_page = self.extract_text_with_ocr(pdf_path)

        elif method == 'text':
            text_by_page = self.extract_text_pymupdf(pdf_path)

        elif method == 'ocr':
            if not self.use_ocr:
                raise ValueError("OCR is disabled but OCR method was requested")
            text_by_page = self.extract_text_with_ocr(pdf_path)

        else:
            raise ValueError(f"Unknown method: {method}")

        # Update stats
        self.stats['total_pages'] = len(text_by_page)
        self.stats['end_time'] = datetime.now().isoformat()

        # Format output
        output = self.format_output(text_by_page, pdf_path)

        # Save if output path provided
        if output_path:
            self.save_output(output, output_path)

        # Print summary
        self.print_summary()

        return output

    def format_output(self, text_by_page: Dict[int, str],
                      source_path: str) -> str:
        """
        Format extracted text based on output format

        Args:
            text_by_page: Dictionary of page numbers to text
            source_path: Path to source PDF

        Returns:
            Formatted output string
        """
        if self.output_format == 'json':
            data = {
                'source': source_path,
                'timestamp': datetime.now().isoformat(),
                'stats': self.stats,
                'pages': text_by_page
            }
            return json.dumps(data, indent=2, ensure_ascii=False)

        elif self.output_format == 'markdown':
            output = [f"# PDF Transcription\n"]
            output.append(f"**Source:** `{source_path}`\n")
            output.append(f"**Date:** {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}\n")
            output.append(f"**Total Pages:** {len(text_by_page)}\n")
            output.append("\n---\n")

            for page_num in sorted(text_by_page.keys()):
                output.append(f"\n## Page {page_num}\n")
                output.append(text_by_page[page_num])
                output.append("\n---\n")

            return '\n'.join(output)

        else:  # text format
            output = []
            output.append(f"{'=' * 80}")
            output.append(f"PDF Transcription: {source_path}")
            output.append(f"Date: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
            output.append(f"Total Pages: {len(text_by_page)}")
            output.append(f"{'=' * 80}\n")

            for page_num in sorted(text_by_page.keys()):
                output.append(f"\n{'=' * 40}")
                output.append(f"PAGE {page_num}")
                output.append(f"{'=' * 40}\n")
                output.append(text_by_page[page_num])

            return '\n'.join(output)

    def save_output(self, output: str, output_path: str):
        """
        Save output to file

        Args:
            output: Formatted output string
            output_path: Path to save file
        """
        try:
            with open(output_path, 'w', encoding='utf-8') as f:
                f.write(output)
            print(f"\n✅ Output saved to: {output_path}")
        except Exception as e:
            print(f"❌ Error saving output: {e}")

    def print_summary(self):
        """Print transcription summary"""
        print("\n" + "=" * 60)
        print("TRANSCRIPTION SUMMARY")
        print("=" * 60)
        print(f"Total pages processed: {self.stats['total_pages']}")
        print(f"Pages with text extraction: {self.stats['text_pages']}")
        print(f"Pages processed with OCR: {self.stats['ocr_pages']}")

        if self.stats['errors']:
            print(f"\n⚠️  Errors encountered: {len(self.stats['errors'])}")
            for error in self.stats['errors'][:5]:  # Show first 5 errors
                print(f"  - {error}")
        else:
            print("\n✅ No errors encountered")
        print("=" * 60)


def main():
    """Main CLI interface"""
    parser = argparse.ArgumentParser(
        description='Extract text from PDF documents (supports both text and scanned PDFs)'
    )

    parser.add_argument(
        'input_pdf',
        help='Path to input PDF file'
    )

    parser.add_argument(
        '-o', '--output',
        help='Output file path (if not specified, prints to stdout)'
    )

    parser.add_argument(
        '-m', '--method',
        choices=['auto', 'text', 'ocr'],
        default='auto',
        help='Extraction method (default: auto)'
    )

    parser.add_argument(
        '-f', '--format',
        choices=['text', 'json', 'markdown'],
        default='text',
        help='Output format (default: text)'
    )

    parser.add_argument(
        '--ocr-lang',
        default='eng',
        help='OCR language (default: eng). Use "eng+fra" for multiple languages'
    )

    parser.add_argument(
        '--dpi',
        type=int,
        default=300,
        help='DPI for OCR image conversion (default: 300)'
    )

    parser.add_argument(
        '--no-ocr',
        action='store_true',
        help='Disable OCR (only use text extraction)'
    )

    parser.add_argument(
        '--pages',
        help='Page range to process (e.g., "1-5" or "1,3,5")'
    )

    args = parser.parse_args()

    # Create transcriber
    transcriber = PDFTranscriber(
        use_ocr=not args.no_ocr,
        ocr_language=args.ocr_lang,
        dpi=args.dpi,
        output_format=args.format
    )

    try:
        # Transcribe PDF
        result = transcriber.transcribe(
            pdf_path=args.input_pdf,
            output_path=args.output,
            method=args.method
        )

        # Print to stdout if no output file specified
        if not args.output:
            print("\n" + "=" * 60)
            print("EXTRACTED TEXT")
            print("=" * 60)
            print(result)

    except Exception as e:
        print(f"\n❌ Error: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()