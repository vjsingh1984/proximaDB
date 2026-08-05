from __future__ import annotations

import hashlib
import importlib.util
import struct
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "corpus_io", ROOT / "scripts/bench/corpus_io.py"
)
assert SPEC and SPEC.loader
CORPUS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CORPUS)


def _write_vecs(path: Path, vectors: list[list[float]], fmt: str) -> None:
    with open(path, "wb") as handle:
        for vector in vectors:
            handle.write(struct.pack("<i", len(vector)))
            handle.write(struct.pack(f"<{len(vector)}{fmt}", *vector))


class CorpusIoTest(unittest.TestCase):
    def test_read_fvecs_roundtrip(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "base.fvecs"
            _write_vecs(path, [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]], "f")
            self.assertEqual(
                list(CORPUS.read_fvecs(path)),
                [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
            )

    def test_read_ivecs_roundtrip(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "gt.ivecs"
            _write_vecs(path, [[7, 8], [9, 10]], "i")
            self.assertEqual(list(CORPUS.read_ivecs(path)), [[7, 8], [9, 10]])

    def test_read_fvecs_truncated_body_raises(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "bad.fvecs"
            with open(path, "wb") as handle:
                handle.write(struct.pack("<i", 3))  # claims dim 3 …
                handle.write(b"\x00\x00\x00\x00")  # … but only one float follows
            with self.assertRaises(ValueError):
                list(CORPUS.read_fvecs(path))

    def test_sha256_file_and_verify_checksum(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "blob.bin"
            path.write_bytes(b"hello")
            expected = hashlib.sha256(b"hello").hexdigest()
            self.assertEqual(CORPUS.sha256_file(path), expected)
            self.assertEqual(CORPUS.verify_checksum(path, expected), expected)
            with self.assertRaises(ValueError):
                CORPUS.verify_checksum(path, "0" * 64)

    def test_fetch_corpus_downloads_verifies_and_is_idempotent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            dest = Path(directory) / "nested" / "corpus.fvecs"
            payload = b"vectors-payload"
            checksum = hashlib.sha256(payload).hexdigest()
            calls: list[str] = []

            def fake_download(url: str, tmp: str) -> None:
                calls.append(url)
                Path(tmp).write_bytes(payload)

            got = CORPUS.fetch_corpus(
                "http://example/corpus", dest, checksum, download=fake_download
            )
            self.assertEqual(got, checksum)
            self.assertTrue(dest.exists())
            self.assertEqual(len(calls), 1)

            # An existing file with the right checksum is reused, not re-downloaded.
            again = CORPUS.fetch_corpus(
                "http://example/corpus", dest, checksum, download=fake_download
            )
            self.assertEqual(again, checksum)
            self.assertEqual(len(calls), 1)

    def test_fetch_corpus_checksum_mismatch_raises_and_cleans_up(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            dest = Path(directory) / "corpus.fvecs"

            def fake_download(url: str, tmp: str) -> None:
                Path(tmp).write_bytes(b"corrupted")

            with self.assertRaises(ValueError):
                CORPUS.fetch_corpus(
                    "http://example", dest, "0" * 64, download=fake_download
                )
            self.assertFalse(dest.exists())
            self.assertFalse(dest.with_name(dest.name + ".part").exists())


if __name__ == "__main__":
    unittest.main()
