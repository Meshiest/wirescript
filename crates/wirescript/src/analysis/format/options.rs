//! Formatter options, and reading them from `.editorconfig`.

use std::path::Path;

/// The print width when nothing sets one: lines wider than this are split
/// where the formatter can.
pub const DEFAULT_PRINT_WIDTH: usize = 100;

/// How lines end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }

    /// The ending most of `source`'s lines use (LF on a tie or with none).
    pub fn dominant_in(source: &str) -> Self {
        let crlf = source.matches("\r\n").count();
        let lf = source.matches('\n').count() - crlf;
        if crlf > lf { Self::Crlf } else { Self::Lf }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatOptions {
    /// One level of indentation: a tab or some spaces.
    pub indent: String,
    /// Lines wider than this are split where the formatter can; `None` never
    /// splits.
    pub print_width: Option<usize>,
    /// `None` keeps whichever ending most of the input's lines use.
    pub end_of_line: Option<LineEnding>,
    /// End the file with a line ending. When false, it ends with one only if
    /// the input did.
    pub insert_final_newline: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            indent: "  ".to_string(),
            print_width: Some(DEFAULT_PRINT_WIDTH),
            end_of_line: None,
            insert_final_newline: true,
        }
    }
}

impl FormatOptions {
    /// Indent with a tab, or with `size` spaces.
    #[must_use]
    pub fn indent_with(mut self, tabs: bool, size: usize) -> Self {
        self.indent = if tabs {
            "\t".to_string()
        } else {
            " ".repeat(size)
        };
        self
    }

    /// These options overridden by whatever the `.editorconfig` files that
    /// govern `file` set: `indent_style`, `indent_size`/`tab_width`,
    /// `max_line_length` (`off` never splits), `end_of_line` (`cr` is not
    /// supported and is ignored) and `insert_final_newline`. Properties the
    /// files leave unset, and a file that cannot be read, change nothing.
    /// Trailing whitespace is always trimmed outside `fmt-ignore` lines and
    /// nested-prefab blocks, so `trim_trailing_whitespace` is not read.
    #[must_use]
    pub fn with_editorconfig(mut self, file: &Path) -> Self {
        use ec4rs::property::{
            EndOfLine, FinalNewline, IndentSize, IndentStyle, MaxLineLen, TabWidth,
        };
        let file = if file.is_absolute() {
            file.to_path_buf()
        } else {
            match std::env::current_dir() {
                Ok(dir) => dir.join(file),
                Err(_) => return self,
            }
        };
        let Ok(mut props) = ec4rs::properties_of(&file) else {
            return self;
        };
        props.use_fallbacks();

        let tabs = match props.get::<IndentStyle>() {
            Ok(IndentStyle::Tabs) => Some(true),
            Ok(IndentStyle::Spaces) => Some(false),
            Err(_) => None,
        };
        let size = match props.get::<IndentSize>() {
            Ok(IndentSize::Value(n)) => Some(n),
            Ok(IndentSize::UseTabWidth) | Err(_) => match props.get::<TabWidth>() {
                Ok(TabWidth::Value(n)) => Some(n),
                Err(_) => None,
            },
        };
        let current_tabs = self.indent.starts_with('\t');
        match tabs {
            Some(true) => self.indent = "\t".to_string(),
            Some(false) => {
                let n = size.unwrap_or(if current_tabs { 4 } else { self.indent.len() });
                self.indent = " ".repeat(n);
            }
            // A size alone resizes space indentation; it does not turn tabs
            // into spaces.
            None => {
                if let Some(n) = size
                    && !current_tabs
                {
                    self.indent = " ".repeat(n);
                }
            }
        }
        match props.get::<MaxLineLen>() {
            Ok(MaxLineLen::Value(n)) => self.print_width = Some(n),
            Ok(MaxLineLen::Off) => self.print_width = None,
            Err(_) => {}
        }
        match props.get::<EndOfLine>() {
            Ok(EndOfLine::Lf) => self.end_of_line = Some(LineEnding::Lf),
            Ok(EndOfLine::CrLf) => self.end_of_line = Some(LineEnding::Crlf),
            Ok(EndOfLine::Cr) | Err(_) => {}
        }
        if let Ok(FinalNewline::Value(v)) = props.get::<FinalNewline>() {
            self.insert_final_newline = v;
        }
        self
    }
}
