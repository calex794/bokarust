//! A reading place is a private cursor, never a purchase or an access grant.
#![forbid(unsafe_code)]

/// At most 200 stable anchors per immutable chapter version. Splitting and
/// grouping is deterministic and does not depend on viewport dimensions.
pub fn reader_blocks(text: &str) -> Vec<String> {
    let paragraphs: Vec<&str> = text.split("\n\n").collect();
    let group_size = paragraphs.len().div_ceil(200);
    paragraphs
        .chunks(group_size)
        .map(|group| group.join("\n\n"))
        .collect()
}

#[derive(Clone, Debug)]
pub struct ContinueReading {
    pub book_id: i64,
    pub book_title: String,
    pub chapter_id: i64,
    pub chapter_no: i32,
    pub chapter_title: String,
    /// Zero when the saved version is no longer current; never invent a
    /// position in edited text.
    pub block_index: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgressError {
    Unavailable,
    NotFound,
    StaleVersion,
    Invalid,
}

pub fn valid_place(block_index: i32, block_count: usize) -> Result<(), ProgressError> {
    if block_index <= 0 || usize::try_from(block_index).ok().filter(|n| *n <= block_count).is_none() {
        return Err(ProgressError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_cover_entire_text_without_loss() {
        let source = (0..1200).map(|i| format!("Текст {i}")).collect::<Vec<_>>().join("\n\n");
        let blocks = reader_blocks(&source);
        assert_eq!(blocks.len(), 200);
        assert_eq!(blocks.join("\n\n"), source);
    }

    #[test]
    fn blocks_keep_original_html_as_data() {
        assert_eq!(reader_blocks("a\n\n<script>x</script>"), vec!["a", "<script>x</script>"]);
        assert_eq!(reader_blocks("single").len(), 1);
        assert!(valid_place(1, 1).is_ok());
        assert_eq!(valid_place(0, 1), Err(ProgressError::Invalid));
        assert_eq!(valid_place(201, 200), Err(ProgressError::Invalid));
    }
}