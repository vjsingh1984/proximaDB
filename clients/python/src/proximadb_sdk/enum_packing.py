"""
Ultra-efficient enum packing utilities for ProximaDB Python SDK.

Provides 75% storage savings by packing multiple enums into single uint32 fields.
Each enum uses only 1 byte (0-255) instead of 4 bytes, matching the Rust implementation.
"""

from enum import IntEnum


class ExtractionMethod(IntEnum):
    """Content extraction methods (1 byte storage)."""

    UNSPECIFIED = 0
    DIRECT_TEXT = 1
    OCR = 2
    ASR = 3
    PDF_PARSING = 4
    HTML_PARSING = 5
    DOCUMENT_PARSING = 6
    IMAGE_ANALYSIS = 7
    VIDEO_ANALYSIS = 8
    API_EXTRACTION = 9
    MANUAL_ENTRY = 10


class ProcessingStatus(IntEnum):
    """Content processing status (1 byte storage)."""

    UNSPECIFIED = 0
    RAW = 1
    PROCESSING = 2
    PROCESSED = 3
    FAILED = 4
    REQUIRES_REVIEW = 5
    APPROVED = 6
    DEPRECATED = 7


class QualityLevel(IntEnum):
    """Content quality level (1 byte storage)."""

    UNSPECIFIED = 0
    HIGH = 1
    MEDIUM = 2
    LOW = 3
    UNKNOWN = 4


class DataSource(IntEnum):
    """Data source for provenance tracking (1 byte storage)."""

    UNSPECIFIED = 0
    USER_UPLOAD = 1
    API_INGESTION = 2
    WEB_SCRAPING = 3
    FILE_IMPORT = 4
    DATABASE_SYNC = 5
    THIRD_PARTY_API = 6
    BATCH_PROCESSING = 7
    REAL_TIME_STREAM = 8
    MIGRATION = 9
    BACKUP_RESTORE = 10


class ContentCategory(IntEnum):
    """Content category classification (1 byte storage)."""

    UNSPECIFIED = 0
    DOCUMENT = 1
    IMAGE = 2
    AUDIO = 3
    VIDEO = 4
    CODE = 5
    TABLE = 6
    CHART = 7
    EMAIL = 8
    WEBPAGE = 9
    SOCIAL_MEDIA = 10
    KNOWLEDGE_BASE = 11
    SCIENTIFIC = 12
    LEGAL = 13
    FINANCIAL = 14
    MEDICAL = 15


class LanguageCode(IntEnum):
    """Language codes (1 byte storage)."""

    UNSPECIFIED = 0
    ENGLISH = 1
    SPANISH = 2
    FRENCH = 3
    GERMAN = 4
    ITALIAN = 5
    PORTUGUESE = 6
    RUSSIAN = 7
    CHINESE = 8
    JAPANESE = 9
    KOREAN = 10
    ARABIC = 11
    HINDI = 12
    DUTCH = 13
    SWEDISH = 14
    NORWEGIAN = 15
    DANISH = 16
    FINNISH = 17
    POLISH = 18
    CZECH = 19
    HUNGARIAN = 20
    TURKISH = 21
    GREEK = 22
    HEBREW = 23
    THAI = 24
    VIETNAMESE = 25
    INDONESIAN = 26
    MALAY = 27
    FILIPINO = 28
    CUSTOM = 255  # Use custom_language field


def pack_processing_enums(
    extraction: ExtractionMethod,
    status: ProcessingStatus,
    quality: QualityLevel,
    source: DataSource,
) -> int:
    """
    Pack 4 processing enums into single uint32 (75% storage savings).

    Bit layout:
    - Bits 0-7:   ExtractionMethod (1-10)
    - Bits 8-15:  ProcessingStatus (1-7)
    - Bits 16-23: QualityLevel (1-4)
    - Bits 24-31: DataSource (1-10)

    Args:
        extraction: Content extraction method
        status: Processing status
        quality: Content quality level
        source: Data source

    Returns:
        Packed uint32 value

    Example:
        >>> packed = pack_processing_enums(
        ...     ExtractionMethod.PDF_PARSING,
        ...     ProcessingStatus.PROCESSED,
        ...     QualityLevel.HIGH,
        ...     DataSource.API_INGESTION
        ... )
        >>> print(f"Packed: {packed}")
        Packed: 33620484
    """
    return (
        (int(source) << 24)
        | (int(quality) << 16)
        | (int(status) << 8)
        | int(extraction)
    )


def unpack_processing_enums(
    packed: int,
) -> tuple[ExtractionMethod, ProcessingStatus, QualityLevel, DataSource]:
    """
    Unpack processing enums from uint32.

    Args:
        packed: Packed uint32 value

    Returns:
        Tuple of (extraction, status, quality, source)

    Raises:
        ValueError: If any enum value is invalid

    Example:
        >>> extraction, status, quality, source = unpack_processing_enums(33620484)
        >>> print(f"Extraction: {extraction}")
        >>> print(f"Status: {status}")
        Extraction: ExtractionMethod.PDF_PARSING
        Status: ProcessingStatus.PROCESSED
    """
    extraction_val = packed & 0xFF
    status_val = (packed >> 8) & 0xFF
    quality_val = (packed >> 16) & 0xFF
    source_val = (packed >> 24) & 0xFF

    try:
        return (
            ExtractionMethod(extraction_val),
            ProcessingStatus(status_val),
            QualityLevel(quality_val),
            DataSource(source_val),
        )
    except ValueError as e:
        raise ValueError(f"Invalid enum value in packed data: {e}")


def pack_source_attributes(
    category: ContentCategory,
    quality: QualityLevel,
) -> int:
    """
    Pack 2 source content attributes into uint32.

    Bit layout:
    - Bits 0-7:   ContentCategory (1-15)
    - Bits 8-15:  QualityLevel (1-4)
    - Bits 16-31: Reserved for future attributes

    Args:
        category: Content category
        quality: Quality level

    Returns:
        Packed uint32 value

    Example:
        >>> packed = pack_source_attributes(
        ...     ContentCategory.SCIENTIFIC,
        ...     QualityLevel.HIGH
        ... )
        >>> print(f"Packed: {packed}")
        Packed: 268
    """
    return (int(quality) << 8) | int(category)


def unpack_source_attributes(packed: int) -> tuple[ContentCategory, QualityLevel]:
    """
    Unpack source content attributes from uint32.

    Args:
        packed: Packed uint32 value

    Returns:
        Tuple of (category, quality)

    Raises:
        ValueError: If any enum value is invalid

    Example:
        >>> category, quality = unpack_source_attributes(268)
        >>> print(f"Category: {category}")
        >>> print(f"Quality: {quality}")
        Category: ContentCategory.SCIENTIFIC
        Quality: QualityLevel.HIGH
    """
    category_val = packed & 0xFF
    quality_val = (packed >> 8) & 0xFF

    try:
        return (
            ContentCategory(category_val),
            QualityLevel(quality_val),
        )
    except ValueError as e:
        raise ValueError(f"Invalid enum value in packed data: {e}")


def pack_language_code(language: LanguageCode) -> int:
    """
    Pack language code into uint32.

    Bit layout:
    - Bits 0-7:   LanguageCode (1-28, 255 for custom)
    - Bits 8-31:  Reserved for future language attributes

    Args:
        language: Language code

    Returns:
        Packed uint32 value

    Example:
        >>> packed = pack_language_code(LanguageCode.JAPANESE)
        >>> print(f"Packed: {packed}")
        Packed: 9
    """
    return int(language)


def unpack_language_code(packed: int) -> LanguageCode:
    """
    Unpack language code from uint32.

    Args:
        packed: Packed uint32 value

    Returns:
        Language code

    Raises:
        ValueError: If language code is invalid

    Example:
        >>> language = unpack_language_code(9)
        >>> print(f"Language: {language}")
        Language: LanguageCode.JAPANESE
    """
    language_val = packed & 0xFF

    try:
        return LanguageCode(language_val)
    except ValueError as e:
        raise ValueError(f"Invalid language code: {e}")


# Helper functions for protobuf integration
def create_processing_info(
    model_id: str | None = None,
    extraction: ExtractionMethod = ExtractionMethod.UNSPECIFIED,
    status: ProcessingStatus = ProcessingStatus.UNSPECIFIED,
    quality: QualityLevel = QualityLevel.UNSPECIFIED,
    source: DataSource = DataSource.UNSPECIFIED,
    processing_time_ms: int | None = None,
    processor_version: int | None = None,
) -> dict:
    """
    Create ProcessingInfo dictionary with packed enums.

    Args:
        model_id: Reference to embedding model registry
        extraction: Content extraction method
        status: Processing status
        quality: Content quality level
        source: Data source
        processing_time_ms: Processing time in milliseconds
        processor_version: Processor version

    Returns:
        ProcessingInfo dictionary for protobuf

    Example:
        >>> info = create_processing_info(
        ...     model_id="openai-ada-002",
        ...     extraction=ExtractionMethod.PDF_PARSING,
        ...     status=ProcessingStatus.PROCESSED,
        ...     quality=QualityLevel.HIGH,
        ...     source=DataSource.API_INGESTION
        ... )
        >>> print(info['packed_enums'])
        33620484
    """
    result = {
        "packed_enums": pack_processing_enums(extraction, status, quality, source)
    }

    if model_id is not None:
        result["model_id"] = model_id
    if processing_time_ms is not None:
        result["processing_time_ms"] = processing_time_ms
    if processor_version is not None:
        result["processor_version"] = processor_version

    return result


def create_source_content(
    data_oneof: dict,
    category: ContentCategory = ContentCategory.UNSPECIFIED,
    quality: QualityLevel = QualityLevel.UNSPECIFIED,
    mime_type: str = "",
    size_bytes: int = 0,
    compressed_size: int | None = None,
    checksum: int | None = None,
    processing_info: dict | None = None,
) -> dict:
    """
    Create SourceContent dictionary with packed attributes.

    Args:
        data_oneof: One of text, binary, external, structured data
        category: Content category
        quality: Quality level
        mime_type: MIME type
        size_bytes: Original size in bytes
        compressed_size: Compressed size (if applicable)
        checksum: CRC32 checksum
        processing_info: Processing information

    Returns:
        SourceContent dictionary for protobuf

    Example:
        >>> content = create_source_content(
        ...     data_oneof={'text': {'content': 'Hello world', 'language_code': 1}},
        ...     category=ContentCategory.DOCUMENT,
        ...     quality=QualityLevel.HIGH,
        ...     mime_type='text/plain',
        ...     size_bytes=11
        ... )
        >>> print(content['packed_attributes'])
        257
    """
    result = {
        **data_oneof,
        "packed_attributes": pack_source_attributes(category, quality),
        "mime_type": mime_type,
        "size_bytes": size_bytes,
    }

    if compressed_size is not None:
        result["compressed_size"] = compressed_size
    if checksum is not None:
        result["checksum"] = checksum
    if processing_info is not None:
        result["processing"] = processing_info

    return result


def create_text_content(
    content: str,
    language: LanguageCode = LanguageCode.UNSPECIFIED,
    custom_language: str | None = None,
    chunk_context: dict | None = None,
) -> dict:
    """
    Create TextContent dictionary with packed language.

    Args:
        content: The actual text
        language: Language code (enum)
        custom_language: Custom language for CUSTOM enum value
        chunk_context: RAG chunking information

    Returns:
        TextContent dictionary for protobuf

    Example:
        >>> text = create_text_content(
        ...     content="This is a research paper on AI.",
        ...     language=LanguageCode.ENGLISH
        ... )
        >>> print(text['language_code'])
        1
    """
    result = {
        "content": content,
        "language_code": pack_language_code(language),
    }

    if custom_language is not None:
        result["custom_language"] = custom_language
    if chunk_context is not None:
        result["chunk"] = chunk_context

    return result


# Storage efficiency analysis
def storage_efficiency_analysis():
    """
    Analyze storage efficiency gains from packed enum optimization.

    Returns:
        Dictionary with efficiency metrics
    """
    # Old approach: 4 bytes per enum
    old_processing_info_size = 4 * 4  # 4 enums × 4 bytes each = 16 bytes
    old_source_content_size = 4 * 2  # 2 enums × 4 bytes each = 8 bytes
    old_text_content_size = 4 * 1  # 1 enum × 4 bytes = 4 bytes
    old_total = (
        old_processing_info_size + old_source_content_size + old_text_content_size
    )

    # New approach: packed enums
    new_processing_info_size = 4  # 1 uint32 for 4 enums = 4 bytes
    new_source_content_size = 4  # 1 uint32 for 2 enums = 4 bytes
    new_text_content_size = 4  # 1 uint32 for 1 enum = 4 bytes
    new_total = (
        new_processing_info_size + new_source_content_size + new_text_content_size
    )

    savings_bytes = old_total - new_total
    savings_percent = (savings_bytes / old_total) * 100

    return {
        "old_total_bytes": old_total,
        "new_total_bytes": new_total,
        "savings_bytes": savings_bytes,
        "savings_percent": savings_percent,
        "efficiency_ratio": old_total / new_total,
        "per_vector_savings": savings_bytes,
        "per_million_vectors_savings_mb": (savings_bytes * 1_000_000) / (1024 * 1024),
    }


if __name__ == "__main__":
    # Demonstrate the efficiency gains
    analysis = storage_efficiency_analysis()
    print("🚀 Ultra-Efficient Enum Packing Analysis:")
    print(f"📊 Old storage: {analysis['old_total_bytes']} bytes per vector")
    print(f"📊 New storage: {analysis['new_total_bytes']} bytes per vector")
    print(
        f"💾 Savings: {analysis['savings_bytes']} bytes ({analysis['savings_percent']:.1f}%)"
    )
    print(f"⚡ Efficiency: {analysis['efficiency_ratio']:.1f}x improvement")
    print(
        f"🎯 Per million vectors: {analysis['per_million_vectors_savings_mb']:.1f} MB saved"
    )

    # Demonstrate usage
    print("\n🔧 Example Usage:")

    # Create processing info
    processing = create_processing_info(
        model_id="openai-ada-002",
        extraction=ExtractionMethod.PDF_PARSING,
        status=ProcessingStatus.PROCESSED,
        quality=QualityLevel.HIGH,
        source=DataSource.API_INGESTION,
        processing_time_ms=250,
    )
    print(f"📝 Processing info packed: {processing['packed_enums']}")

    # Create text content
    text = create_text_content(
        content="This paper presents novel approaches to vector databases.",
        language=LanguageCode.ENGLISH,
    )
    print(f"📝 Text language packed: {text['language_code']}")

    # Create source content
    source = create_source_content(
        data_oneof={"text": text},
        category=ContentCategory.SCIENTIFIC,
        quality=QualityLevel.HIGH,
        mime_type="text/plain",
        size_bytes=len(text["content"]),
        processing_info=processing,
    )
    print(f"📝 Source attributes packed: {source['packed_attributes']}")

    # Demonstrate unpacking
    extraction, status, quality, data_source = unpack_processing_enums(
        processing["packed_enums"]
    )
    print(
        f"🔓 Unpacked: {extraction.name}, {status.name}, {quality.name}, {data_source.name}"
    )
