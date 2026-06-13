"""Offline unit tests for proximadb_sdk.enum_packing.

Pure module — no transports or heavy deps. Covers pack/unpack round-trips for
every enum + invalid-value branches + the protobuf helper builders.
"""

import pytest

from proximadb_sdk.enum_packing import (
    ContentCategory,
    DataSource,
    ExtractionMethod,
    LanguageCode,
    ProcessingStatus,
    QualityLevel,
    create_processing_info,
    create_source_content,
    create_text_content,
    pack_language_code,
    pack_processing_enums,
    pack_source_attributes,
    storage_efficiency_analysis,
    unpack_language_code,
    unpack_processing_enums,
    unpack_source_attributes,
)


# ---------------------------------------------------------------------------
# Processing enums: round-trip every member of every enum
# ---------------------------------------------------------------------------
def test_pack_processing_enums_known_vector():
    packed = pack_processing_enums(
        ExtractionMethod.PDF_PARSING,
        ProcessingStatus.PROCESSED,
        QualityLevel.HIGH,
        DataSource.API_INGESTION,
    )
    # source(2)<<24 | quality(1)<<16 | status(3)<<8 | extraction(4)
    assert packed == (2 << 24) | (1 << 16) | (3 << 8) | 4


def test_processing_enums_round_trip_all_members():
    for extraction in ExtractionMethod:
        for status in ProcessingStatus:
            for quality in QualityLevel:
                for source in DataSource:
                    packed = pack_processing_enums(extraction, status, quality, source)
                    assert isinstance(packed, int)
                    e, st, q, src = unpack_processing_enums(packed)
                    assert e == extraction
                    assert st == status
                    assert q == quality
                    assert src == source


def test_unpack_processing_enums_bit_layout():
    # extraction bits 0-7, status 8-15, quality 16-23, source 24-31
    packed = (
        (int(DataSource.MIGRATION) << 24)
        | (int(QualityLevel.LOW) << 16)
        | (int(ProcessingStatus.FAILED) << 8)
        | int(ExtractionMethod.OCR)
    )
    e, st, q, src = unpack_processing_enums(packed)
    assert e is ExtractionMethod.OCR
    assert st is ProcessingStatus.FAILED
    assert q is QualityLevel.LOW
    assert src is DataSource.MIGRATION


def test_unpack_processing_enums_invalid_raises():
    # 200 is not a valid ExtractionMethod
    with pytest.raises(ValueError, match="Invalid enum value in packed data"):
        unpack_processing_enums(200)


def test_unpack_processing_enums_invalid_in_high_byte():
    # source byte = 200 (invalid DataSource), rest valid
    packed = (200 << 24) | (int(QualityLevel.HIGH) << 16)
    with pytest.raises(ValueError, match="Invalid enum value in packed data"):
        unpack_processing_enums(packed)


# ---------------------------------------------------------------------------
# Source attributes
# ---------------------------------------------------------------------------
def test_pack_source_attributes_known_vector():
    packed = pack_source_attributes(ContentCategory.SCIENTIFIC, QualityLevel.HIGH)
    assert packed == 268


def test_source_attributes_round_trip_all_members():
    for category in ContentCategory:
        for quality in QualityLevel:
            packed = pack_source_attributes(category, quality)
            cat, q = unpack_source_attributes(packed)
            assert cat == category
            assert q == quality


def test_unpack_source_attributes_invalid_category():
    with pytest.raises(ValueError, match="Invalid enum value in packed data"):
        unpack_source_attributes(99)  # 99 not a ContentCategory


def test_unpack_source_attributes_invalid_quality():
    # category=1 (valid DOCUMENT), quality byte = 99 (invalid)
    packed = (99 << 8) | int(ContentCategory.DOCUMENT)
    with pytest.raises(ValueError, match="Invalid enum value in packed data"):
        unpack_source_attributes(packed)


# ---------------------------------------------------------------------------
# Language code
# ---------------------------------------------------------------------------
def test_pack_language_code_known_vector():
    assert pack_language_code(LanguageCode.JAPANESE) == 9


def test_language_code_round_trip_all_members():
    for lang in LanguageCode:
        packed = pack_language_code(lang)
        assert unpack_language_code(packed) == lang


def test_unpack_language_code_custom():
    assert unpack_language_code(255) is LanguageCode.CUSTOM


def test_unpack_language_code_only_low_byte_used():
    # High bytes reserved/ignored; low byte determines value
    packed = (0xABCD << 8) | int(LanguageCode.GERMAN)
    assert unpack_language_code(packed) is LanguageCode.GERMAN


def test_unpack_language_code_invalid_raises():
    with pytest.raises(ValueError, match="Invalid language code"):
        unpack_language_code(100)  # 100 not a LanguageCode


# ---------------------------------------------------------------------------
# create_processing_info
# ---------------------------------------------------------------------------
def test_create_processing_info_minimal():
    info = create_processing_info()
    assert info["packed_enums"] == 0
    assert "model_id" not in info
    assert "processing_time_ms" not in info
    assert "processor_version" not in info


def test_create_processing_info_full():
    info = create_processing_info(
        model_id="openai-ada-002",
        extraction=ExtractionMethod.PDF_PARSING,
        status=ProcessingStatus.PROCESSED,
        quality=QualityLevel.HIGH,
        source=DataSource.API_INGESTION,
        processing_time_ms=250,
        processor_version=3,
    )
    assert info["packed_enums"] == (2 << 24) | (1 << 16) | (3 << 8) | 4
    assert info["model_id"] == "openai-ada-002"
    assert info["processing_time_ms"] == 250
    assert info["processor_version"] == 3


# ---------------------------------------------------------------------------
# create_text_content
# ---------------------------------------------------------------------------
def test_create_text_content_minimal():
    text = create_text_content(content="hello", language=LanguageCode.ENGLISH)
    assert text["content"] == "hello"
    assert text["language_code"] == 1
    assert "custom_language" not in text
    assert "chunk" not in text


def test_create_text_content_full():
    chunk = {"index": 0}
    text = create_text_content(
        content="bonjour",
        language=LanguageCode.CUSTOM,
        custom_language="klingon",
        chunk_context=chunk,
    )
    assert text["language_code"] == 255
    assert text["custom_language"] == "klingon"
    assert text["chunk"] is chunk


# ---------------------------------------------------------------------------
# create_source_content
# ---------------------------------------------------------------------------
def test_create_source_content_minimal():
    content = create_source_content(
        data_oneof={"text": {"content": "Hello world", "language_code": 1}},
        category=ContentCategory.DOCUMENT,
        quality=QualityLevel.HIGH,
        mime_type="text/plain",
        size_bytes=11,
    )
    assert content["packed_attributes"] == 257
    assert content["mime_type"] == "text/plain"
    assert content["size_bytes"] == 11
    assert content["text"] == {"content": "Hello world", "language_code": 1}
    assert "compressed_size" not in content
    assert "checksum" not in content
    assert "processing" not in content


def test_create_source_content_full():
    proc = create_processing_info(status=ProcessingStatus.PROCESSED)
    content = create_source_content(
        data_oneof={"binary": b"\x00\x01"},
        category=ContentCategory.SCIENTIFIC,
        quality=QualityLevel.MEDIUM,
        mime_type="application/octet-stream",
        size_bytes=2,
        compressed_size=1,
        checksum=0xDEADBEEF,
        processing_info=proc,
    )
    assert content["packed_attributes"] == pack_source_attributes(
        ContentCategory.SCIENTIFIC, QualityLevel.MEDIUM
    )
    assert content["compressed_size"] == 1
    assert content["checksum"] == 0xDEADBEEF
    assert content["processing"] is proc
    assert content["binary"] == b"\x00\x01"


# ---------------------------------------------------------------------------
# storage_efficiency_analysis
# ---------------------------------------------------------------------------
def test_storage_efficiency_analysis():
    a = storage_efficiency_analysis()
    assert a["old_total_bytes"] == 28
    assert a["new_total_bytes"] == 12
    assert a["savings_bytes"] == 16
    assert a["savings_percent"] == pytest.approx((16 / 28) * 100)
    assert a["efficiency_ratio"] == pytest.approx(28 / 12)
    assert a["per_vector_savings"] == 16
    assert a["per_million_vectors_savings_mb"] == pytest.approx(
        (16 * 1_000_000) / (1024 * 1024)
    )
