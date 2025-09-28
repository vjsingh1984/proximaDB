#!/usr/bin/env python3
"""
Safe PDF Transcriber Wrapper
Ensures PDF files are never modified during transcription
"""

import os
import sys
import shutil
import tempfile
import hashlib
from pathlib import Path
import argparse

# Import the main transcriber
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pdf_transcriber import PDFTranscriber


class SafePDFTranscriber:
    """
    Safe wrapper for PDF transcription that guarantees no file modifications
    """

    def __init__(self):
        self.transcriber = None

    def verify_file_integrity(self, file_path: str) -> str:
        """
        Calculate MD5 hash of file for integrity verification

        Args:
            file_path: Path to file

        Returns:
            MD5 hash string
        """
        md5_hash = hashlib.md5()
        with open(file_path, "rb") as f:
            # Read file in chunks to handle large files
            for chunk in iter(lambda: f.read(4096), b""):
                md5_hash.update(chunk)
        return md5_hash.hexdigest()

    def transcribe_safe(self, pdf_path: str, output_path: str = None,
                       use_copy: bool = True, **kwargs):
        """
        Safely transcribe PDF with integrity checks

        Args:
            pdf_path: Path to original PDF
            output_path: Path for output text file
            use_copy: Whether to work on a copy of the PDF
            **kwargs: Additional arguments for PDFTranscriber

        Returns:
            Transcribed text
        """
        # Check if file exists
        if not os.path.exists(pdf_path):
            raise FileNotFoundError(f"PDF file not found: {pdf_path}")

        # Get original file stats
        original_path = os.path.abspath(pdf_path)
        original_stats = os.stat(original_path)
        original_hash = self.verify_file_integrity(original_path)
        original_mtime = original_stats.st_mtime

        print(f"📄 Original file: {original_path}")
        print(f"🔒 File hash: {original_hash}")
        print(f"📊 File size: {original_stats.st_size:,} bytes")

        # Store original permissions
        original_mode = original_stats.st_mode

        # Work path (either copy or original)
        work_path = original_path
        temp_dir = None

        try:
            if use_copy:
                # Create temporary copy to work with
                temp_dir = tempfile.mkdtemp(prefix="pdf_transcriber_")
                temp_pdf = os.path.join(temp_dir, os.path.basename(pdf_path))
                shutil.copy2(original_path, temp_pdf)
                work_path = temp_pdf
                print(f"🔄 Working on copy: {work_path}")

            # Make absolutely sure we're in read-only mode
            if not use_copy:
                # Temporarily set file to read-only
                os.chmod(original_path, 0o444)
                print("🔒 Set file to read-only mode")

            # Create transcriber
            self.transcriber = PDFTranscriber(**kwargs)

            # Perform transcription
            print("\n📝 Starting transcription...")
            result = self.transcriber.transcribe(
                work_path,
                output_path=output_path,
                method=kwargs.get('method', 'auto')
            )

            return result

        finally:
            # Clean up temporary directory if created
            if temp_dir and os.path.exists(temp_dir):
                shutil.rmtree(temp_dir)
                print("🧹 Cleaned up temporary files")

            # Restore original permissions if we changed them
            if not use_copy and os.path.exists(original_path):
                os.chmod(original_path, original_mode)
                print("🔓 Restored original file permissions")

            # Verify file integrity
            if os.path.exists(original_path):
                final_hash = self.verify_file_integrity(original_path)
                final_stats = os.stat(original_path)
                final_mtime = final_stats.st_mtime

                print("\n✅ Integrity Check:")
                print(f"  Original hash: {original_hash}")
                print(f"  Final hash:    {final_hash}")
                print(f"  Hash match:    {'✅ YES' if original_hash == final_hash else '❌ NO'}")
                print(f"  Size unchanged: {'✅ YES' if original_stats.st_size == final_stats.st_size else '❌ NO'}")
                print(f"  Modified time:  {'✅ UNCHANGED' if original_mtime == final_mtime else '⚠️ CHANGED'}")

                if original_hash != final_hash:
                    print("\n⚠️ WARNING: File hash changed! This should not happen.")
                    print("Please check the original file for unintended modifications.")


def main():
    """Main CLI interface for safe PDF transcription"""
    parser = argparse.ArgumentParser(
        description='Safely extract text from PDF documents with integrity verification'
    )

    parser.add_argument(
        'input_pdf',
        help='Path to input PDF file'
    )

    parser.add_argument(
        '-o', '--output',
        help='Output file path for transcribed text'
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
        help='OCR language (default: eng)'
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
        help='Disable OCR'
    )

    parser.add_argument(
        '--no-copy',
        action='store_true',
        help='Work directly on original file (not recommended)'
    )

    parser.add_argument(
        '--verify-only',
        action='store_true',
        help='Only verify file integrity without transcription'
    )

    args = parser.parse_args()

    # Create safe transcriber
    safe_transcriber = SafePDFTranscriber()

    if args.verify_only:
        # Just verify file integrity
        try:
            hash_value = safe_transcriber.verify_file_integrity(args.input_pdf)
            stats = os.stat(args.input_pdf)
            print(f"File: {args.input_pdf}")
            print(f"MD5 Hash: {hash_value}")
            print(f"Size: {stats.st_size:,} bytes")
            print(f"Readable: {'Yes' if os.access(args.input_pdf, os.R_OK) else 'No'}")
            print(f"Writable: {'Yes' if os.access(args.input_pdf, os.W_OK) else 'No'}")
        except Exception as e:
            print(f"Error verifying file: {e}")
            sys.exit(1)
    else:
        # Perform safe transcription
        try:
            result = safe_transcriber.transcribe_safe(
                pdf_path=args.input_pdf,
                output_path=args.output,
                use_copy=not args.no_copy,
                use_ocr=not args.no_ocr,
                ocr_language=args.ocr_lang,
                dpi=args.dpi,
                output_format=args.format,
                method=args.method
            )

            # Print result if no output file specified
            if not args.output:
                print("\n" + "=" * 60)
                print("TRANSCRIBED TEXT")
                print("=" * 60)
                print(result)

        except Exception as e:
            print(f"\n❌ Error during transcription: {e}")
            sys.exit(1)


if __name__ == "__main__":
    main()