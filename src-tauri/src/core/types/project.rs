//! Project type — the detected primary language/ecosystem of a workspace.

/// The detected primary language/ecosystem of a project.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProjectType {
    /// Rust project using Cargo.
    Rust,
    /// Node.js project using npm or yarn.
    NodeNpm,
    /// Node.js project using pnpm.
    NodePnpm,
    /// Node.js project using bun.
    NodeBun,
    /// Python project using poetry.
    PythonPoetry,
    /// Python project using pip + requirements.txt.
    PythonPip,
    /// Python project using uv.
    PythonUv,
    /// Go project using go.mod.
    Go,
    /// Java project using Maven.
    JavaMaven,
    /// Java project using Gradle.
    JavaGradle,
    /// C/C++ project using CMake.
    Cmake,
    /// .NET project using .csproj.
    Dotnet,
    /// Mixed-language monorepo (detected by workspace markers).
    Monorepo,
    /// Unknown project type.
    Unknown,
}

impl ProjectType {
    /// Human-readable label for event payloads and logging.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::NodeNpm => "node_npm",
            Self::NodePnpm => "node_pnpm",
            Self::NodeBun => "node_bun",
            Self::PythonPoetry => "python_poetry",
            Self::PythonPip => "python_pip",
            Self::PythonUv => "python_uv",
            Self::Go => "go",
            Self::JavaMaven => "java_maven",
            Self::JavaGradle => "java_gradle",
            Self::Cmake => "cmake",
            Self::Dotnet => "dotnet",
            Self::Monorepo => "monorepo",
            Self::Unknown => "unknown",
        }
    }

    /// File extensions this project type primarily uses.
    pub fn source_extensions(&self) -> &'static [&'static str] {
        match self {
            Self::Rust => &["rs"],
            Self::NodeNpm | Self::NodePnpm | Self::NodeBun => &["ts", "tsx", "js", "jsx", "mjs"],
            Self::PythonPoetry | Self::PythonPip | Self::PythonUv => &["py"],
            Self::Go => &["go"],
            Self::JavaMaven | Self::JavaGradle => &["java", "kt"],
            Self::Cmake => &["c", "cpp", "cc", "h", "hpp"],
            Self::Dotnet => &["cs"],
            Self::Monorepo => &["rs", "ts", "py", "go"],
            Self::Unknown => &[],
        }
    }
}
