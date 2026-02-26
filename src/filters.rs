use glob::Pattern;

pub struct FileFilter {
    include_patterns: Vec<Pattern>,
    exclude_patterns: Vec<Pattern>,
}

impl FileFilter {
    /// Create a new FileFilter with include and exclude patterns
    pub fn new(include: Vec<String>, exclude: Vec<String>) -> Result<Self, String> {
        let include_patterns = include
            .into_iter()
            .map(|p| Pattern::new(&p).map_err(|e| format!("Invalid include pattern: {}", e)))
            .collect::<Result<Vec<_>, _>>()?;

        let exclude_patterns = exclude
            .into_iter()
            .map(|p| Pattern::new(&p).map_err(|e| format!("Invalid exclude pattern: {}", e)))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(FileFilter {
            include_patterns,
            exclude_patterns,
        })
    }

    /// Check if a file path should be included based on filter rules
    /// Returns true if the file passes the filters
    ///
    /// Rules:
    /// 1. If exclude pattern matches, return false (exclude takes precedence)
    /// 2. If include patterns exist and none match, return false
    /// 3. Otherwise return true
    pub fn matches(&self, path: &str) -> bool {
        // Check exclude patterns first (they take precedence)
        for pattern in &self.exclude_patterns {
            if pattern.matches(path) {
                return false;
            }
        }

        // If there are include patterns, at least one must match
        if !self.include_patterns.is_empty() {
            for pattern in &self.include_patterns {
                if pattern.matches(path) {
                    return true;
                }
            }
            return false;
        }

        // No include patterns, and not excluded
        true
    }

    /// Check if any filters are set
    #[allow(dead_code)]
    pub fn has_filters(&self) -> bool {
        !self.include_patterns.is_empty() || !self.exclude_patterns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_include_only() {
        let filter = FileFilter::new(vec!["*.txt".to_string()], vec![]).unwrap();
        assert!(filter.matches("file.txt"));
        assert!(!filter.matches("file.rs"));
    }

    #[test]
    fn test_exclude_only() {
        let filter = FileFilter::new(vec![], vec!["*.log".to_string()]).unwrap();
        assert!(filter.matches("file.txt"));
        assert!(!filter.matches("file.log"));
    }

    #[test]
    fn test_exclude_precedence() {
        let filter =
            FileFilter::new(vec!["*.txt".to_string()], vec!["secret*.txt".to_string()]).unwrap();
        assert!(filter.matches("file.txt"));
        assert!(!filter.matches("secret.txt"));
        assert!(!filter.matches("secret_key.txt"));
    }

    #[test]
    fn test_no_filters() {
        let filter = FileFilter::new(vec![], vec![]).unwrap();
        assert!(filter.matches("any_file.txt"));
        assert!(filter.matches("any_file.rs"));
    }
}
