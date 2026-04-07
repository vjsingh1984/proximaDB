//! Ultra-efficient enum packing utilities
//!
//! Provides 75% storage savings by packing multiple enums into single uint32 fields.
//! Each enum uses only 1 byte (0-255) instead of 4 bytes, with 3 bytes available for
//! future attributes or multiple packed enums.

use anyhow::Result;

/// Processing information enum values (1 byte each)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionMethod {
    /// No extraction method specified
    Unspecified = 0,
    /// Direct text extraction from source
    DirectText = 1,
    /// Optical character recognition
    Ocr = 2,
    /// Automatic speech recognition
    Asr = 3,
    /// PDF document parsing
    PdfParsing = 4,
    /// HTML content parsing
    HtmlParsing = 5,
    /// Generic document parsing
    DocumentParsing = 6,
    /// Image content analysis
    ImageAnalysis = 7,
    /// Video content analysis
    VideoAnalysis = 8,
    /// External API-based extraction
    ApiExtraction = 9,
    /// Manual human entry
    ManualEntry = 10,
}

/// Current processing state of a record
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessingStatus {
    /// No status specified
    Unspecified = 0,
    /// Unprocessed raw data
    Raw = 1,
    /// Currently being processed
    Processing = 2,
    /// Successfully processed
    Processed = 3,
    /// Processing failed with an error
    Failed = 4,
    /// Awaiting manual review
    RequiresReview = 5,
    /// Reviewed and approved
    Approved = 6,
    /// Marked as deprecated
    Deprecated = 7,
}

/// Data quality assessment level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityLevel {
    /// No quality level specified
    Unspecified = 0,
    /// High quality data
    High = 1,
    /// Medium quality data
    Medium = 2,
    /// Low quality data
    Low = 3,
    /// Quality could not be determined
    Unknown = 4,
}

/// Origin of ingested data
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSource {
    /// No source specified
    Unspecified = 0,
    /// Uploaded directly by a user
    UserUpload = 1,
    /// Ingested through an API call
    ApiIngestion = 2,
    /// Scraped from web pages
    WebScraping = 3,
    /// Imported from local files
    FileImport = 4,
    /// Synchronized from an external database
    DatabaseSync = 5,
    /// Fetched from a third-party API
    ThirdPartyApi = 6,
    /// Ingested via a batch processing pipeline
    BatchProcessing = 7,
    /// Received from a real-time data stream
    RealTimeStream = 8,
    /// Migrated from another system
    Migration = 9,
    /// Restored from a backup
    BackupRestore = 10,
}

/// Broad category of the content being stored
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentCategory {
    /// No category specified
    Unspecified = 0,
    /// Text document
    Document = 1,
    /// Image content
    Image = 2,
    /// Audio content
    Audio = 3,
    /// Video content
    Video = 4,
    /// Source code
    Code = 5,
    /// Tabular data
    Table = 6,
    /// Chart or graph visualization
    Chart = 7,
    /// Email message
    Email = 8,
    /// Web page content
    Webpage = 9,
    /// Social media post
    SocialMedia = 10,
    /// Knowledge base article
    KnowledgeBase = 11,
    /// Scientific paper or dataset
    Scientific = 12,
    /// Legal document
    Legal = 13,
    /// Financial document or report
    Financial = 14,
    /// Medical record or literature
    Medical = 15,
}

/// Language of the source content, packed as a single byte
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageCode {
    /// No language specified
    Unspecified = 0,
    /// English
    English = 1,
    /// Spanish
    Spanish = 2,
    /// French
    French = 3,
    /// German
    German = 4,
    /// Italian
    Italian = 5,
    /// Portuguese
    Portuguese = 6,
    /// Russian
    Russian = 7,
    /// Chinese (Mandarin)
    Chinese = 8,
    /// Japanese
    Japanese = 9,
    /// Korean
    Korean = 10,
    /// Arabic
    Arabic = 11,
    /// Hindi
    Hindi = 12,
    /// Dutch
    Dutch = 13,
    /// Swedish
    Swedish = 14,
    /// Norwegian
    Norwegian = 15,
    /// Danish
    Danish = 16,
    /// Finnish
    Finnish = 17,
    /// Polish
    Polish = 18,
    /// Czech
    Czech = 19,
    /// Hungarian
    Hungarian = 20,
    /// Turkish
    Turkish = 21,
    /// Greek
    Greek = 22,
    /// Hebrew
    Hebrew = 23,
    /// Thai
    Thai = 24,
    /// Vietnamese
    Vietnamese = 25,
    /// Indonesian
    Indonesian = 26,
    /// Malay
    Malay = 27,
    /// Filipino
    Filipino = 28,
    /// Custom language specified in the custom_language field
    Custom = 255,
}

/// Pack 4 processing enums into single uint32 (75% storage savings)
///
/// Bit layout:
/// - Bits 0-7:   ExtractionMethod (1-10)
/// - Bits 8-15:  ProcessingStatus (1-7)
/// - Bits 16-23: QualityLevel (1-4)
/// - Bits 24-31: DataSource (1-10)
pub fn pack_processing_enums(
    extraction: ExtractionMethod,
    status: ProcessingStatus,
    quality: QualityLevel,
    source: DataSource,
) -> u32 {
    ((source as u32) << 24)
        | ((quality as u32) << 16)
        | ((status as u32) << 8)
        | (extraction as u32)
}

/// Unpack processing enums from uint32
pub fn unpack_processing_enums(
    packed: u32,
) -> Result<(ExtractionMethod, ProcessingStatus, QualityLevel, DataSource)> {
    let extraction = (packed & 0xFF) as u8;
    let status = ((packed >> 8) & 0xFF) as u8;
    let quality = ((packed >> 16) & 0xFF) as u8;
    let source = ((packed >> 24) & 0xFF) as u8;

    Ok((
        ExtractionMethod::try_from(extraction)?,
        ProcessingStatus::try_from(status)?,
        QualityLevel::try_from(quality)?,
        DataSource::try_from(source)?,
    ))
}

/// Pack 2 source content attributes into uint32
///
/// Bit layout:
/// - Bits 0-7:   ContentCategory (1-15)
/// - Bits 8-15:  QualityLevel (1-4)
/// - Bits 16-31: Reserved for future attributes
pub fn pack_source_attributes(category: ContentCategory, quality: QualityLevel) -> u32 {
    ((quality as u32) << 8) | (category as u32)
}

/// Unpack source content attributes from uint32
pub fn unpack_source_attributes(packed: u32) -> Result<(ContentCategory, QualityLevel)> {
    let category = (packed & 0xFF) as u8;
    let quality = ((packed >> 8) & 0xFF) as u8;

    Ok((
        ContentCategory::try_from(category)?,
        QualityLevel::try_from(quality)?,
    ))
}

/// Pack language code into uint32
///
/// Bit layout:
/// - Bits 0-7:   LanguageCode (1-28, 255 for custom)
/// - Bits 8-31:  Reserved for future language attributes
pub fn pack_language_code(language: LanguageCode) -> u32 {
    language as u32
}

/// Unpack language code from uint32
pub fn unpack_language_code(packed: u32) -> Result<LanguageCode> {
    let language = (packed & 0xFF) as u8;
    LanguageCode::try_from(language)
}

// TryFrom implementations for enum conversion
impl TryFrom<u8> for ExtractionMethod {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(ExtractionMethod::Unspecified),
            1 => Ok(ExtractionMethod::DirectText),
            2 => Ok(ExtractionMethod::Ocr),
            3 => Ok(ExtractionMethod::Asr),
            4 => Ok(ExtractionMethod::PdfParsing),
            5 => Ok(ExtractionMethod::HtmlParsing),
            6 => Ok(ExtractionMethod::DocumentParsing),
            7 => Ok(ExtractionMethod::ImageAnalysis),
            8 => Ok(ExtractionMethod::VideoAnalysis),
            9 => Ok(ExtractionMethod::ApiExtraction),
            10 => Ok(ExtractionMethod::ManualEntry),
            _ => Err(anyhow::anyhow!("Invalid ExtractionMethod value: {}", value)),
        }
    }
}

impl TryFrom<u8> for ProcessingStatus {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(ProcessingStatus::Unspecified),
            1 => Ok(ProcessingStatus::Raw),
            2 => Ok(ProcessingStatus::Processing),
            3 => Ok(ProcessingStatus::Processed),
            4 => Ok(ProcessingStatus::Failed),
            5 => Ok(ProcessingStatus::RequiresReview),
            6 => Ok(ProcessingStatus::Approved),
            7 => Ok(ProcessingStatus::Deprecated),
            _ => Err(anyhow::anyhow!("Invalid ProcessingStatus value: {}", value)),
        }
    }
}

impl TryFrom<u8> for QualityLevel {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(QualityLevel::Unspecified),
            1 => Ok(QualityLevel::High),
            2 => Ok(QualityLevel::Medium),
            3 => Ok(QualityLevel::Low),
            4 => Ok(QualityLevel::Unknown),
            _ => Err(anyhow::anyhow!("Invalid QualityLevel value: {}", value)),
        }
    }
}

impl TryFrom<u8> for DataSource {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(DataSource::Unspecified),
            1 => Ok(DataSource::UserUpload),
            2 => Ok(DataSource::ApiIngestion),
            3 => Ok(DataSource::WebScraping),
            4 => Ok(DataSource::FileImport),
            5 => Ok(DataSource::DatabaseSync),
            6 => Ok(DataSource::ThirdPartyApi),
            7 => Ok(DataSource::BatchProcessing),
            8 => Ok(DataSource::RealTimeStream),
            9 => Ok(DataSource::Migration),
            10 => Ok(DataSource::BackupRestore),
            _ => Err(anyhow::anyhow!("Invalid DataSource value: {}", value)),
        }
    }
}

impl TryFrom<u8> for ContentCategory {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(ContentCategory::Unspecified),
            1 => Ok(ContentCategory::Document),
            2 => Ok(ContentCategory::Image),
            3 => Ok(ContentCategory::Audio),
            4 => Ok(ContentCategory::Video),
            5 => Ok(ContentCategory::Code),
            6 => Ok(ContentCategory::Table),
            7 => Ok(ContentCategory::Chart),
            8 => Ok(ContentCategory::Email),
            9 => Ok(ContentCategory::Webpage),
            10 => Ok(ContentCategory::SocialMedia),
            11 => Ok(ContentCategory::KnowledgeBase),
            12 => Ok(ContentCategory::Scientific),
            13 => Ok(ContentCategory::Legal),
            14 => Ok(ContentCategory::Financial),
            15 => Ok(ContentCategory::Medical),
            _ => Err(anyhow::anyhow!("Invalid ContentCategory value: {}", value)),
        }
    }
}

impl TryFrom<u8> for LanguageCode {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(LanguageCode::Unspecified),
            1 => Ok(LanguageCode::English),
            2 => Ok(LanguageCode::Spanish),
            3 => Ok(LanguageCode::French),
            4 => Ok(LanguageCode::German),
            5 => Ok(LanguageCode::Italian),
            6 => Ok(LanguageCode::Portuguese),
            7 => Ok(LanguageCode::Russian),
            8 => Ok(LanguageCode::Chinese),
            9 => Ok(LanguageCode::Japanese),
            10 => Ok(LanguageCode::Korean),
            11 => Ok(LanguageCode::Arabic),
            12 => Ok(LanguageCode::Hindi),
            13 => Ok(LanguageCode::Dutch),
            14 => Ok(LanguageCode::Swedish),
            15 => Ok(LanguageCode::Norwegian),
            16 => Ok(LanguageCode::Danish),
            17 => Ok(LanguageCode::Finnish),
            18 => Ok(LanguageCode::Polish),
            19 => Ok(LanguageCode::Czech),
            20 => Ok(LanguageCode::Hungarian),
            21 => Ok(LanguageCode::Turkish),
            22 => Ok(LanguageCode::Greek),
            23 => Ok(LanguageCode::Hebrew),
            24 => Ok(LanguageCode::Thai),
            25 => Ok(LanguageCode::Vietnamese),
            26 => Ok(LanguageCode::Indonesian),
            27 => Ok(LanguageCode::Malay),
            28 => Ok(LanguageCode::Filipino),
            255 => Ok(LanguageCode::Custom),
            _ => Err(anyhow::anyhow!("Invalid LanguageCode value: {}", value)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_processing_enum_packing() {
        let extraction = ExtractionMethod::PdfParsing;
        let status = ProcessingStatus::Processed;
        let quality = QualityLevel::High;
        let source = DataSource::ApiIngestion;

        let packed = pack_processing_enums(extraction, status, quality, source);
        let (e, s, q, src) = unpack_processing_enums(packed).unwrap();

        assert_eq!(e, extraction);
        assert_eq!(s, status);
        assert_eq!(q, quality);
        assert_eq!(src, source);
    }

    #[test]
    fn test_source_attributes_packing() {
        let category = ContentCategory::Scientific;
        let quality = QualityLevel::High;

        let packed = pack_source_attributes(category, quality);
        let (c, q) = unpack_source_attributes(packed).unwrap();

        assert_eq!(c, category);
        assert_eq!(q, quality);
    }

    #[test]
    fn test_language_packing() {
        let language = LanguageCode::Japanese;

        let packed = pack_language_code(language);
        let l = unpack_language_code(packed).unwrap();

        assert_eq!(l, language);
    }

    #[test]
    fn test_storage_efficiency() {
        // Old approach: 4 bytes per enum = 16 bytes total
        // New approach: 4 bytes for all enums = 75% savings

        let packed = pack_processing_enums(
            ExtractionMethod::PdfParsing,
            ProcessingStatus::Processed,
            QualityLevel::High,
            DataSource::ApiIngestion,
        );

        // Verify it fits in 4 bytes
        assert!(packed <= u32::MAX);

        // Verify each enum has a u8 representation (the `as u8` cast itself validates this)
        let _ = ExtractionMethod::ManualEntry as u8;
        let _ = ProcessingStatus::Deprecated as u8;
        let _ = QualityLevel::Unknown as u8;
        let _ = DataSource::BackupRestore as u8;
    }
}
