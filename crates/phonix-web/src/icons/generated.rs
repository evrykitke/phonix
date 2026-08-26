//! Icon geometry, generated from Lucide v1.33.0 (ISC).
//!
//! DO NOT EDIT. Add a name to `tools/icons.txt` and run:
//!
//! ```text
//! npm install lucide-static
//! node tools/generate-icons.mjs
//! ```
//!
//! Each body is the inside of Lucide's 24x24 `<svg>`, with the wrapper
//! stripped - see [`super::Icon`] for why.

/// Every icon the application can draw.
///
/// The variants are the whole vocabulary: a screen cannot reach for an icon
/// that has not been through `tools/icons.txt`, which is what keeps the bundle
/// from quietly growing an icon at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Icon {
    /// Lucide `arrow-left`.
    ArrowLeft,
    /// Lucide `arrow-right`.
    ArrowRight,
    /// Lucide `ban`.
    Ban,
    /// Lucide `bell`.
    Bell,
    /// Lucide `blocks`.
    Blocks,
    /// Lucide `bold`.
    Bold,
    /// Lucide `boxes`.
    Boxes,
    /// Lucide `building-2`.
    Building2,
    /// Lucide `calendar`.
    Calendar,
    /// Lucide `chart-column`.
    ChartColumn,
    /// Lucide `check`.
    Check,
    /// Lucide `chevron-down`.
    ChevronDown,
    /// Lucide `chevron-left`.
    ChevronLeft,
    /// Lucide `chevron-right`.
    ChevronRight,
    /// Lucide `chevrons-up-down`.
    ChevronsUpDown,
    /// Lucide `chevron-up`.
    ChevronUp,
    /// Lucide `circle-alert`.
    CircleAlert,
    /// Lucide `circle-check`.
    CircleCheck,
    /// Lucide `circle-help`.
    CircleHelp,
    /// Lucide `circle-user`.
    CircleUser,
    /// Lucide `clipboard-list`.
    ClipboardList,
    /// Lucide `clock`.
    Clock,
    /// Lucide `command`.
    Command,
    /// Lucide `copy`.
    Copy,
    /// Lucide `corner-down-left`.
    CornerDownLeft,
    /// Lucide `crop`.
    Crop,
    /// Lucide `download`.
    Download,
    /// Lucide `ellipsis`.
    Ellipsis,
    /// Lucide `external-link`.
    ExternalLink,
    /// Lucide `eye`.
    Eye,
    /// Lucide `eye-off`.
    EyeOff,
    /// Lucide `file-text`.
    FileText,
    /// Lucide `filter`.
    Filter,
    /// Lucide `heading-2`.
    Heading2,
    /// Lucide `heading-3`.
    Heading3,
    /// Lucide `image`.
    Image,
    /// Lucide `info`.
    Info,
    /// Lucide `italic`.
    Italic,
    /// Lucide `key-round`.
    KeyRound,
    /// Lucide `key-square`.
    KeySquare,
    /// Lucide `languages`.
    Languages,
    /// Lucide `layout-dashboard`.
    LayoutDashboard,
    /// Lucide `layout-grid`.
    LayoutGrid,
    /// Lucide `link`.
    Link,
    /// Lucide `link-2-off`.
    Link2Off,
    /// Lucide `list`.
    List,
    /// Lucide `list-ordered`.
    ListOrdered,
    /// Lucide `list-tree`.
    ListTree,
    /// Lucide `loader-circle`.
    LoaderCircle,
    /// Lucide `lock`.
    Lock,
    /// Lucide `log-out`.
    LogOut,
    /// Lucide `mail`.
    Mail,
    /// Lucide `menu`.
    Menu,
    /// Lucide `minus`.
    Minus,
    /// Lucide `monitor`.
    Monitor,
    /// Lucide `moon`.
    Moon,
    /// Lucide `package`.
    Package,
    /// Lucide `palette`.
    Palette,
    /// Lucide `panel-left-close`.
    PanelLeftClose,
    /// Lucide `panel-left-open`.
    PanelLeftOpen,
    /// Lucide `pencil`.
    Pencil,
    /// Lucide `plus`.
    Plus,
    /// Lucide `qr-code`.
    QrCode,
    /// Lucide `receipt`.
    Receipt,
    /// Lucide `redo-2`.
    Redo2,
    /// Lucide `refresh-cw`.
    RefreshCw,
    /// Lucide `remove-formatting`.
    RemoveFormatting,
    /// Lucide `save`.
    Save,
    /// Lucide `scroll-text`.
    ScrollText,
    /// Lucide `search`.
    Search,
    /// Lucide `settings`.
    Settings,
    /// Lucide `shield`.
    Shield,
    /// Lucide `shield-check`.
    ShieldCheck,
    /// Lucide `shield-off`.
    ShieldOff,
    /// Lucide `shopping-cart`.
    ShoppingCart,
    /// Lucide `sliders-horizontal`.
    SlidersHorizontal,
    /// Lucide `smartphone`.
    Smartphone,
    /// Lucide `strikethrough`.
    Strikethrough,
    /// Lucide `sun`.
    Sun,
    /// Lucide `table`.
    Table,
    /// Lucide `text-quote`.
    TextQuote,
    /// Lucide `trash-2`.
    Trash2,
    /// Lucide `triangle-alert`.
    TriangleAlert,
    /// Lucide `truck`.
    Truck,
    /// Lucide `underline`.
    Underline,
    /// Lucide `undo-2`.
    Undo2,
    /// Lucide `upload`.
    Upload,
    /// Lucide `user`.
    User,
    /// Lucide `user-plus`.
    UserPlus,
    /// Lucide `users`.
    Users,
    /// Lucide `warehouse`.
    Warehouse,
    /// Lucide `x`.
    X,
}

impl Icon {
    /// The icon's Lucide name, e.g. `"chevron-right"`.
    ///
    /// Stable enough to persist: it is what a data-driven menu would store.
    pub const fn key(self) -> &'static str {
        match self {
            Self::ArrowLeft => "arrow-left",
            Self::ArrowRight => "arrow-right",
            Self::Ban => "ban",
            Self::Bell => "bell",
            Self::Blocks => "blocks",
            Self::Bold => "bold",
            Self::Boxes => "boxes",
            Self::Building2 => "building-2",
            Self::Calendar => "calendar",
            Self::ChartColumn => "chart-column",
            Self::Check => "check",
            Self::ChevronDown => "chevron-down",
            Self::ChevronLeft => "chevron-left",
            Self::ChevronRight => "chevron-right",
            Self::ChevronsUpDown => "chevrons-up-down",
            Self::ChevronUp => "chevron-up",
            Self::CircleAlert => "circle-alert",
            Self::CircleCheck => "circle-check",
            Self::CircleHelp => "circle-help",
            Self::CircleUser => "circle-user",
            Self::ClipboardList => "clipboard-list",
            Self::Clock => "clock",
            Self::Command => "command",
            Self::Copy => "copy",
            Self::CornerDownLeft => "corner-down-left",
            Self::Crop => "crop",
            Self::Download => "download",
            Self::Ellipsis => "ellipsis",
            Self::ExternalLink => "external-link",
            Self::Eye => "eye",
            Self::EyeOff => "eye-off",
            Self::FileText => "file-text",
            Self::Filter => "filter",
            Self::Heading2 => "heading-2",
            Self::Heading3 => "heading-3",
            Self::Image => "image",
            Self::Info => "info",
            Self::Italic => "italic",
            Self::KeyRound => "key-round",
            Self::KeySquare => "key-square",
            Self::Languages => "languages",
            Self::LayoutDashboard => "layout-dashboard",
            Self::LayoutGrid => "layout-grid",
            Self::Link => "link",
            Self::Link2Off => "link-2-off",
            Self::List => "list",
            Self::ListOrdered => "list-ordered",
            Self::ListTree => "list-tree",
            Self::LoaderCircle => "loader-circle",
            Self::Lock => "lock",
            Self::LogOut => "log-out",
            Self::Mail => "mail",
            Self::Menu => "menu",
            Self::Minus => "minus",
            Self::Monitor => "monitor",
            Self::Moon => "moon",
            Self::Package => "package",
            Self::Palette => "palette",
            Self::PanelLeftClose => "panel-left-close",
            Self::PanelLeftOpen => "panel-left-open",
            Self::Pencil => "pencil",
            Self::Plus => "plus",
            Self::QrCode => "qr-code",
            Self::Receipt => "receipt",
            Self::Redo2 => "redo-2",
            Self::RefreshCw => "refresh-cw",
            Self::RemoveFormatting => "remove-formatting",
            Self::Save => "save",
            Self::ScrollText => "scroll-text",
            Self::Search => "search",
            Self::Settings => "settings",
            Self::Shield => "shield",
            Self::ShieldCheck => "shield-check",
            Self::ShieldOff => "shield-off",
            Self::ShoppingCart => "shopping-cart",
            Self::SlidersHorizontal => "sliders-horizontal",
            Self::Smartphone => "smartphone",
            Self::Strikethrough => "strikethrough",
            Self::Sun => "sun",
            Self::Table => "table",
            Self::TextQuote => "text-quote",
            Self::Trash2 => "trash-2",
            Self::TriangleAlert => "triangle-alert",
            Self::Truck => "truck",
            Self::Underline => "underline",
            Self::Undo2 => "undo-2",
            Self::Upload => "upload",
            Self::User => "user",
            Self::UserPlus => "user-plus",
            Self::Users => "users",
            Self::Warehouse => "warehouse",
            Self::X => "x",
        }
    }

    /// The SVG geometry, without the `<svg>` wrapper.
    pub const fn body(self) -> &'static str {
        match self {
            Self::ArrowLeft => r#"<path d="m12 19-7-7 7-7" /> <path d="M19 12H5" />"#,
            Self::ArrowRight => r#"<path d="M5 12h14" /> <path d="m12 5 7 7-7 7" />"#,
            Self::Ban => r#"<circle cx="12" cy="12" r="10" /> <path d="M4.929 4.929 19.07 19.071" />"#,
            Self::Bell => r#"<path d="M10.268 21a2 2 0 0 0 3.464 0" /> <path d="M3.262 15.326A1 1 0 0 0 4 17h16a1 1 0 0 0 .74-1.673C19.41 13.956 18 12.499 18 8A6 6 0 0 0 6 8c0 4.499-1.411 5.956-2.738 7.326" />"#,
            Self::Blocks => r#"<path d="M10 22V7a1 1 0 0 0-1-1H4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-5a1 1 0 0 0-1-1H2" /> <rect x="14" y="2" width="8" height="8" rx="1" />"#,
            Self::Bold => r#"<path d="M6 12h9a4 4 0 0 1 0 8H7a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h7a4 4 0 0 1 0 8" />"#,
            Self::Boxes => r#"<path d="M2.97 12.92A2 2 0 0 0 2 14.63v3.24a2 2 0 0 0 .97 1.71l3 1.8a2 2 0 0 0 2.06 0L12 19v-5.5l-5-3-4.03 2.42Z" /> <path d="m7 16.5-4.74-2.85" /> <path d="m7 16.5 5-3" /> <path d="M7 16.5v5.17" /> <path d="M12 13.5V19l3.97 2.38a2 2 0 0 0 2.06 0l3-1.8a2 2 0 0 0 .97-1.71v-3.24a2 2 0 0 0-.97-1.71L17 10.5l-5 3Z" /> <path d="m17 16.5-5-3" /> <path d="m17 16.5 4.74-2.85" /> <path d="M17 16.5v5.17" /> <path d="M7.97 4.42A2 2 0 0 0 7 6.13v4.37l5 3 5-3V6.13a2 2 0 0 0-.97-1.71l-3-1.8a2 2 0 0 0-2.06 0l-3 1.8Z" /> <path d="M12 8 7.26 5.15" /> <path d="m12 8 4.74-2.85" /> <path d="M12 13.5V8" />"#,
            Self::Building2 => r#"<path d="M10 12h4" /> <path d="M10 8h4" /> <path d="M14 21v-3a2 2 0 0 0-4 0v3" /> <path d="M6 10H4a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V9a2 2 0 0 0-2-2h-2" /> <path d="M6 21V5a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v16" />"#,
            Self::Calendar => r#"<path d="M8 2v3" /> <path d="M16 2v3" /> <rect x="3" y="3" width="18" height="18" rx="2" /> <path d="M3 9h18" />"#,
            Self::ChartColumn => r#"<path d="M3 3v16a2 2 0 0 0 2 2h16" /> <path d="M18 17V9" /> <path d="M13 17V5" /> <path d="M8 17v-3" />"#,
            Self::Check => r#"<path d="M20 6 9 17l-5-5" />"#,
            Self::ChevronDown => r#"<path d="m6 9 6 6 6-6" />"#,
            Self::ChevronLeft => r#"<path d="m15 18-6-6 6-6" />"#,
            Self::ChevronRight => r#"<path d="m9 18 6-6-6-6" />"#,
            Self::ChevronsUpDown => r#"<path d="m7 15 5 5 5-5" /> <path d="m7 9 5-5 5 5" />"#,
            Self::ChevronUp => r#"<path d="m18 15-6-6-6 6" />"#,
            Self::CircleAlert => r#"<circle cx="12" cy="12" r="10" /> <line x1="12" x2="12" y1="8" y2="12" /> <line x1="12" x2="12.01" y1="16" y2="16" />"#,
            Self::CircleCheck => r#"<circle cx="12" cy="12" r="10" /> <path d="m9 12 2 2 4-4" />"#,
            Self::CircleHelp => r#"<circle cx="12" cy="12" r="10" /> <path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3" /> <path d="M12 17h.01" />"#,
            Self::CircleUser => r#"<circle cx="12" cy="12" r="10" /> <circle cx="12" cy="10" r="3" /> <path d="M7 20.662V19a2 2 0 0 1 2-2h6a2 2 0 0 1 2 2v1.662" />"#,
            Self::ClipboardList => r#"<rect width="8" height="4" x="8" y="2" rx="1" ry="1" /> <path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2" /> <path d="M12 11h4" /> <path d="M12 16h4" /> <path d="M8 11h.01" /> <path d="M8 16h.01" />"#,
            Self::Clock => r#"<circle cx="12" cy="12" r="10" /> <path d="M12 6v6l4 2" />"#,
            Self::Command => r#"<path d="M15 6v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3V6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3" />"#,
            Self::Copy => r#"<rect width="14" height="14" x="8" y="8" rx="2" ry="2" /> <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />"#,
            Self::CornerDownLeft => r#"<path d="M20 4v7a4 4 0 0 1-4 4H4" /> <path d="m9 10-5 5 5 5" />"#,
            Self::Crop => r#"<path d="M6 2v14a2 2 0 0 0 2 2h14" /> <path d="M18 22V8a2 2 0 0 0-2-2H2" />"#,
            Self::Download => r#"<path d="M12 15V3" /> <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" /> <path d="m7 10 5 5 5-5" />"#,
            Self::Ellipsis => r#"<circle cx="12" cy="12" r="1" /> <circle cx="19" cy="12" r="1" /> <circle cx="5" cy="12" r="1" />"#,
            Self::ExternalLink => r#"<path d="M15 3h6v6" /> <path d="M10 14 21 3" /> <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />"#,
            Self::Eye => r#"<path d="M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0" /> <circle cx="12" cy="12" r="3" />"#,
            Self::EyeOff => r#"<path d="M10.733 5.076a10.744 10.744 0 0 1 11.205 6.575 1 1 0 0 1 0 .696 10.747 10.747 0 0 1-1.444 2.49" /> <path d="M14.084 14.158a3 3 0 0 1-4.242-4.242" /> <path d="M17.479 17.499a10.75 10.75 0 0 1-15.417-5.151 1 1 0 0 1 0-.696 10.75 10.75 0 0 1 4.446-5.143" /> <path d="m2 2 20 20" />"#,
            Self::FileText => r#"<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z" /> <path d="M14 2v5a1 1 0 0 0 1 1h5" /> <path d="M10 9H8" /> <path d="M16 13H8" /> <path d="M16 17H8" />"#,
            Self::Filter => r#"<path d="M10 20a1 1 0 0 0 .553.895l2 1A1 1 0 0 0 14 21v-7a2 2 0 0 1 .517-1.341L21.74 4.67A1 1 0 0 0 21 3H3a1 1 0 0 0-.742 1.67l7.225 7.989A2 2 0 0 1 10 14z" />"#,
            Self::Heading2 => r#"<path d="M4 12h8" /> <path d="M4 18V6" /> <path d="M12 18V6" /> <path d="M21 18h-4c0-4 4-3 4-6 0-1.5-2-2.5-4-1" />"#,
            Self::Heading3 => r#"<path d="M4 12h8" /> <path d="M4 18V6" /> <path d="M12 18V6" /> <path d="M17.5 10.5c1.7-1 3.5 0 3.5 1.5a2 2 0 0 1-2 2" /> <path d="M17 17.5c2 1.5 4 .3 4-1.5a2 2 0 0 0-2-2" />"#,
            Self::Image => r#"<rect width="18" height="18" x="3" y="3" rx="2" ry="2" /> <circle cx="9" cy="9" r="2" /> <path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21" />"#,
            Self::Info => r#"<circle cx="12" cy="12" r="10" /> <path d="M12 16v-4" /> <path d="M12 8h.01" />"#,
            Self::Italic => r#"<line x1="19" x2="10" y1="4" y2="4" /> <line x1="14" x2="5" y1="20" y2="20" /> <line x1="15" x2="9" y1="4" y2="20" />"#,
            Self::KeyRound => r#"<path d="M2.586 17.414A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172a2 2 0 0 0 1.414-.586l.814-.814a6.5 6.5 0 1 0-4-4z" /> <circle cx="16.5" cy="7.5" r=".5" fill="currentColor" />"#,
            Self::KeySquare => r#"<path d="M12.4 2.7a2.5 2.5 0 0 1 3.4 0l5.5 5.5a2.5 2.5 0 0 1 0 3.4l-3.7 3.7a2.5 2.5 0 0 1-3.4 0L8.7 9.8a2.5 2.5 0 0 1 0-3.4z" /> <path d="m14 7 3 3" /> <path d="m9.4 10.6-6.814 6.814A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172a2 2 0 0 0 1.414-.586l.814-.814" />"#,
            Self::Languages => r#"<path d="m5 8 6 6" /> <path d="m4 14 6-6 2-3" /> <path d="M2 5h12" /> <path d="M7 2h1" /> <path d="m22 22-5-10-5 10" /> <path d="M14 18h6" />"#,
            Self::LayoutDashboard => r#"<rect width="7" height="9" x="3" y="3" rx="1" /> <rect width="7" height="5" x="14" y="3" rx="1" /> <rect width="7" height="9" x="14" y="12" rx="1" /> <rect width="7" height="5" x="3" y="16" rx="1" />"#,
            Self::LayoutGrid => r#"<rect width="7" height="7" x="3" y="3" rx="1" /> <rect width="7" height="7" x="14" y="3" rx="1" /> <rect width="7" height="7" x="14" y="14" rx="1" /> <rect width="7" height="7" x="3" y="14" rx="1" />"#,
            Self::Link => r#"<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" /> <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />"#,
            Self::Link2Off => r#"<path d="M9 17H7A5 5 0 0 1 7 7" /> <path d="M15 7h2a5 5 0 0 1 4 8" /> <line x1="8" x2="12" y1="12" y2="12" /> <line x1="2" x2="22" y1="2" y2="22" />"#,
            Self::List => r#"<path d="M3 5h.01" /> <path d="M3 12h.01" /> <path d="M3 19h.01" /> <path d="M8 5h13" /> <path d="M8 12h13" /> <path d="M8 19h13" />"#,
            Self::ListOrdered => r#"<path d="M11 5h10" /> <path d="M11 12h10" /> <path d="M11 19h10" /> <path d="M4 4h1v5" /> <path d="M4 9h2" /> <path d="M6.5 20H3.4c0-1 2.6-1.925 2.6-3.5a1.5 1.5 0 0 0-2.6-1.02" />"#,
            Self::ListTree => r#"<path d="M8 5h13" /> <path d="M13 12h8" /> <path d="M13 19h8" /> <path d="M3 10a2 2 0 0 0 2 2h3" /> <path d="M3 5v12a2 2 0 0 0 2 2h3" />"#,
            Self::LoaderCircle => r#"<path d="M21 12a9 9 0 1 1-6.219-8.56" />"#,
            Self::Lock => r#"<rect width="18" height="11" x="3" y="11" rx="2" ry="2" /> <path d="M7 11V7a5 5 0 0 1 10 0v4" />"#,
            Self::LogOut => r#"<path d="m16 17 5-5-5-5" /> <path d="M21 12H9" /> <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />"#,
            Self::Mail => r#"<path d="m22 7-8.991 5.727a2 2 0 0 1-2.009 0L2 7" /> <rect x="2" y="4" width="20" height="16" rx="2" />"#,
            Self::Menu => r#"<path d="M4 5h16" /> <path d="M4 12h16" /> <path d="M4 19h16" />"#,
            Self::Minus => r#"<path d="M5 12h14" />"#,
            Self::Monitor => r#"<rect width="20" height="14" x="2" y="3" rx="2" /> <line x1="8" x2="16" y1="21" y2="21" /> <line x1="12" x2="12" y1="17" y2="21" />"#,
            Self::Moon => r#"<path d="M20.985 12.486a9 9 0 1 1-9.473-9.472c.405-.022.617.46.402.803a6 6 0 0 0 8.268 8.268c.344-.215.825-.004.803.401" />"#,
            Self::Package => r#"<path d="M11 21.73a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73z" /> <path d="M12 22V12" /> <polyline points="3.29 7 12 12 20.71 7" /> <path d="m7.5 4.27 9 5.15" />"#,
            Self::Palette => r#"<path d="M12 22a1 1 0 0 1 0-20 10 9 0 0 1 10 9 5 5 0 0 1-5 5h-2.25a1.75 1.75 0 0 0-1.4 2.8l.3.4a1.75 1.75 0 0 1-1.4 2.8z" /> <circle cx="13.5" cy="6.5" r=".5" fill="currentColor" /> <circle cx="17.5" cy="10.5" r=".5" fill="currentColor" /> <circle cx="6.5" cy="12.5" r=".5" fill="currentColor" /> <circle cx="8.5" cy="7.5" r=".5" fill="currentColor" />"#,
            Self::PanelLeftClose => r#"<rect width="18" height="18" x="3" y="3" rx="2" /> <path d="M9 3v18" /> <path d="m16 15-3-3 3-3" />"#,
            Self::PanelLeftOpen => r#"<rect width="18" height="18" x="3" y="3" rx="2" /> <path d="M9 3v18" /> <path d="m14 9 3 3-3 3" />"#,
            Self::Pencil => r#"<path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z" /> <path d="m15 5 4 4" />"#,
            Self::Plus => r#"<path d="M5 12h14" /> <path d="M12 5v14" />"#,
            Self::QrCode => r#"<rect width="5" height="5" x="3" y="3" rx="1" /> <rect width="5" height="5" x="16" y="3" rx="1" /> <rect width="5" height="5" x="3" y="16" rx="1" /> <path d="M21 16h-3a2 2 0 0 0-2 2v3" /> <path d="M21 21v.01" /> <path d="M12 7v3a2 2 0 0 1-2 2H7" /> <path d="M3 12h.01" /> <path d="M12 3h.01" /> <path d="M12 16v.01" /> <path d="M16 12h1" /> <path d="M21 12v.01" /> <path d="M12 21v-1" />"#,
            Self::Receipt => r#"<path d="M12 17V7" /> <path d="M16 8h-6a2 2 0 0 0 0 4h4a2 2 0 0 1 0 4H8" /> <path d="M4 3a1 1 0 0 1 1-1 1.3 1.3 0 0 1 .7.2l.933.6a1.3 1.3 0 0 0 1.4 0l.934-.6a1.3 1.3 0 0 1 1.4 0l.933.6a1.3 1.3 0 0 0 1.4 0l.933-.6a1.3 1.3 0 0 1 1.4 0l.934.6a1.3 1.3 0 0 0 1.4 0l.933-.6A1.3 1.3 0 0 1 19 2a1 1 0 0 1 1 1v18a1 1 0 0 1-1 1 1.3 1.3 0 0 1-.7-.2l-.933-.6a1.3 1.3 0 0 0-1.4 0l-.934.6a1.3 1.3 0 0 1-1.4 0l-.933-.6a1.3 1.3 0 0 0-1.4 0l-.933.6a1.3 1.3 0 0 1-1.4 0l-.934-.6a1.3 1.3 0 0 0-1.4 0l-.933.6a1.3 1.3 0 0 1-.7.2 1 1 0 0 1-1-1z" />"#,
            Self::Redo2 => r#"<path d="m15 14 5-5-5-5" /> <path d="M20 9H9.5A5.5 5.5 0 0 0 4 14.5A5.5 5.5 0 0 0 9.5 20H13" />"#,
            Self::RefreshCw => r#"<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" /> <path d="M21 3v5h-5" /> <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" /> <path d="M8 16H3v5" />"#,
            Self::RemoveFormatting => r#"<path d="M4 7V4h16v3" /> <path d="M5 20h6" /> <path d="M13 4 8 20" /> <path d="m15 15 5 5" /> <path d="m20 15-5 5" />"#,
            Self::Save => r#"<path d="M15.2 3a2 2 0 0 1 1.4.6l3.8 3.8a2 2 0 0 1 .6 1.4V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z" /> <path d="M17 21v-7a1 1 0 0 0-1-1H8a1 1 0 0 0-1 1v7" /> <path d="M7 3v4a1 1 0 0 0 1 1h7" />"#,
            Self::ScrollText => r#"<path d="M15 12h-5" /> <path d="M15 8h-5" /> <path d="M19 17V5a2 2 0 0 0-2-2H4" /> <path d="M8 21h12a2 2 0 0 0 2-2v-1a1 1 0 0 0-1-1H11a1 1 0 0 0-1 1v1a2 2 0 1 1-4 0V5a2 2 0 1 0-4 0v2a1 1 0 0 0 1 1h3" />"#,
            Self::Search => r#"<path d="m21 21-4.34-4.34" /> <circle cx="11" cy="11" r="8" />"#,
            Self::Settings => r#"<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915" /> <circle cx="12" cy="12" r="3" />"#,
            Self::Shield => r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" />"#,
            Self::ShieldCheck => r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" /> <path d="m9 12 2 2 4-4" />"#,
            Self::ShieldOff => r#"<path d="m2 2 20 20" /> <path d="M5 5a1 1 0 0 0-1 1v7c0 5 3.5 7.5 7.67 8.94a1 1 0 0 0 .67.01c2.35-.82 4.48-1.97 5.9-3.71" /> <path d="M9.309 3.652A12.252 12.252 0 0 0 11.24 2.28a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1v7a9.784 9.784 0 0 1-.08 1.264" />"#,
            Self::ShoppingCart => r#"<circle cx="8" cy="21" r="1" /> <circle cx="19" cy="21" r="1" /> <path d="M2.05 2.05h2l2.66 12.42a2 2 0 0 0 2 1.58h9.78a2 2 0 0 0 1.95-1.57l1.65-7.43H5.12" />"#,
            Self::SlidersHorizontal => r#"<path d="M10 5H3" /> <path d="M12 19H3" /> <path d="M14 3v4" /> <path d="M16 17v4" /> <path d="M21 12h-9" /> <path d="M21 19h-5" /> <path d="M21 5h-7" /> <path d="M8 10v4" /> <path d="M8 12H3" />"#,
            Self::Smartphone => r#"<rect width="14" height="20" x="5" y="2" rx="2" ry="2" /> <path d="M12 18h.01" />"#,
            Self::Strikethrough => r#"<path d="M16 4H9a3 3 0 0 0-2.83 4" /> <path d="M14 12a4 4 0 0 1 0 8H6" /> <line x1="4" x2="20" y1="12" y2="12" />"#,
            Self::Sun => r#"<circle cx="12" cy="12" r="4" /> <path d="M12 2v2" /> <path d="M12 20v2" /> <path d="m4.93 4.93 1.41 1.41" /> <path d="m17.66 17.66 1.41 1.41" /> <path d="M2 12h2" /> <path d="M20 12h2" /> <path d="m6.34 17.66-1.41 1.41" /> <path d="m19.07 4.93-1.41 1.41" />"#,
            Self::Table => r#"<path d="M12 3v18" /> <rect width="18" height="18" x="3" y="3" rx="2" /> <path d="M3 9h18" /> <path d="M3 15h18" />"#,
            Self::TextQuote => r#"<path d="M17 5H3" /> <path d="M21 12H8" /> <path d="M21 19H8" /> <path d="M3 12v7" />"#,
            Self::Trash2 => r#"<path d="M10 11v6" /> <path d="M14 11v6" /> <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" /> <path d="M3 6h18" /> <path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />"#,
            Self::TriangleAlert => r#"<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3" /> <path d="M12 9v4" /> <path d="M12 17h.01" />"#,
            Self::Truck => r#"<path d="M14 18V6a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2v11a1 1 0 0 0 1 1h2" /> <path d="M15 18H9" /> <path d="M19 18h2a1 1 0 0 0 1-1v-3.65a1 1 0 0 0-.22-.624l-3.48-4.35A1 1 0 0 0 17.52 8H14" /> <circle cx="17" cy="18" r="2" /> <circle cx="7" cy="18" r="2" />"#,
            Self::Underline => r#"<path d="M6 4v6a6 6 0 0 0 12 0V4" /> <line x1="4" x2="20" y1="20" y2="20" />"#,
            Self::Undo2 => r#"<path d="M9 14 4 9l5-5" /> <path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11" />"#,
            Self::Upload => r#"<path d="M12 3v12" /> <path d="m17 8-5-5-5 5" /> <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />"#,
            Self::User => r#"<path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2" /> <circle cx="12" cy="7" r="4" />"#,
            Self::UserPlus => r#"<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" /> <circle cx="9" cy="7" r="4" /> <line x1="19" x2="19" y1="8" y2="14" /> <line x1="22" x2="16" y1="11" y2="11" />"#,
            Self::Users => r#"<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" /> <path d="M16 3.128a4 4 0 0 1 0 7.744" /> <path d="M22 21v-2a4 4 0 0 0-3-3.87" /> <circle cx="9" cy="7" r="4" />"#,
            Self::Warehouse => r#"<path d="M18 21V10a1 1 0 0 0-1-1H7a1 1 0 0 0-1 1v11" /> <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V8a2 2 0 0 1 1.132-1.803l7.95-3.974a2 2 0 0 1 1.837 0l7.948 3.974A2 2 0 0 1 22 8z" /> <path d="M6 13h12" /> <path d="M6 17h12" />"#,
            Self::X => r#"<path d="M18 6 6 18" /> <path d="m6 6 12 12" />"#,
        }
    }

    /// Every icon, in variant order. Drives the icon gallery and the tests.
    pub const ALL: &'static [Icon] = &[
        Icon::ArrowLeft,
        Icon::ArrowRight,
        Icon::Ban,
        Icon::Bell,
        Icon::Blocks,
        Icon::Bold,
        Icon::Boxes,
        Icon::Building2,
        Icon::Calendar,
        Icon::ChartColumn,
        Icon::Check,
        Icon::ChevronDown,
        Icon::ChevronLeft,
        Icon::ChevronRight,
        Icon::ChevronsUpDown,
        Icon::ChevronUp,
        Icon::CircleAlert,
        Icon::CircleCheck,
        Icon::CircleHelp,
        Icon::CircleUser,
        Icon::ClipboardList,
        Icon::Clock,
        Icon::Command,
        Icon::Copy,
        Icon::CornerDownLeft,
        Icon::Crop,
        Icon::Download,
        Icon::Ellipsis,
        Icon::ExternalLink,
        Icon::Eye,
        Icon::EyeOff,
        Icon::FileText,
        Icon::Filter,
        Icon::Heading2,
        Icon::Heading3,
        Icon::Image,
        Icon::Info,
        Icon::Italic,
        Icon::KeyRound,
        Icon::KeySquare,
        Icon::Languages,
        Icon::LayoutDashboard,
        Icon::LayoutGrid,
        Icon::Link,
        Icon::Link2Off,
        Icon::List,
        Icon::ListOrdered,
        Icon::ListTree,
        Icon::LoaderCircle,
        Icon::Lock,
        Icon::LogOut,
        Icon::Mail,
        Icon::Menu,
        Icon::Minus,
        Icon::Monitor,
        Icon::Moon,
        Icon::Package,
        Icon::Palette,
        Icon::PanelLeftClose,
        Icon::PanelLeftOpen,
        Icon::Pencil,
        Icon::Plus,
        Icon::QrCode,
        Icon::Receipt,
        Icon::Redo2,
        Icon::RefreshCw,
        Icon::RemoveFormatting,
        Icon::Save,
        Icon::ScrollText,
        Icon::Search,
        Icon::Settings,
        Icon::Shield,
        Icon::ShieldCheck,
        Icon::ShieldOff,
        Icon::ShoppingCart,
        Icon::SlidersHorizontal,
        Icon::Smartphone,
        Icon::Strikethrough,
        Icon::Sun,
        Icon::Table,
        Icon::TextQuote,
        Icon::Trash2,
        Icon::TriangleAlert,
        Icon::Truck,
        Icon::Underline,
        Icon::Undo2,
        Icon::Upload,
        Icon::User,
        Icon::UserPlus,
        Icon::Users,
        Icon::Warehouse,
        Icon::X,
    ];
}

impl core::str::FromStr for Icon {
    type Err = ();

    /// Parse a Lucide name back into a variant.
    ///
    /// For the day menus come out of a database rather than out of
    /// `navigation::tree`: an unknown name is an error, never a blank space.
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "arrow-left" => Ok(Self::ArrowLeft),
            "arrow-right" => Ok(Self::ArrowRight),
            "ban" => Ok(Self::Ban),
            "bell" => Ok(Self::Bell),
            "blocks" => Ok(Self::Blocks),
            "bold" => Ok(Self::Bold),
            "boxes" => Ok(Self::Boxes),
            "building-2" => Ok(Self::Building2),
            "calendar" => Ok(Self::Calendar),
            "chart-column" => Ok(Self::ChartColumn),
            "check" => Ok(Self::Check),
            "chevron-down" => Ok(Self::ChevronDown),
            "chevron-left" => Ok(Self::ChevronLeft),
            "chevron-right" => Ok(Self::ChevronRight),
            "chevrons-up-down" => Ok(Self::ChevronsUpDown),
            "chevron-up" => Ok(Self::ChevronUp),
            "circle-alert" => Ok(Self::CircleAlert),
            "circle-check" => Ok(Self::CircleCheck),
            "circle-help" => Ok(Self::CircleHelp),
            "circle-user" => Ok(Self::CircleUser),
            "clipboard-list" => Ok(Self::ClipboardList),
            "clock" => Ok(Self::Clock),
            "command" => Ok(Self::Command),
            "copy" => Ok(Self::Copy),
            "corner-down-left" => Ok(Self::CornerDownLeft),
            "crop" => Ok(Self::Crop),
            "download" => Ok(Self::Download),
            "ellipsis" => Ok(Self::Ellipsis),
            "external-link" => Ok(Self::ExternalLink),
            "eye" => Ok(Self::Eye),
            "eye-off" => Ok(Self::EyeOff),
            "file-text" => Ok(Self::FileText),
            "filter" => Ok(Self::Filter),
            "heading-2" => Ok(Self::Heading2),
            "heading-3" => Ok(Self::Heading3),
            "image" => Ok(Self::Image),
            "info" => Ok(Self::Info),
            "italic" => Ok(Self::Italic),
            "key-round" => Ok(Self::KeyRound),
            "key-square" => Ok(Self::KeySquare),
            "languages" => Ok(Self::Languages),
            "layout-dashboard" => Ok(Self::LayoutDashboard),
            "layout-grid" => Ok(Self::LayoutGrid),
            "link" => Ok(Self::Link),
            "link-2-off" => Ok(Self::Link2Off),
            "list" => Ok(Self::List),
            "list-ordered" => Ok(Self::ListOrdered),
            "list-tree" => Ok(Self::ListTree),
            "loader-circle" => Ok(Self::LoaderCircle),
            "lock" => Ok(Self::Lock),
            "log-out" => Ok(Self::LogOut),
            "mail" => Ok(Self::Mail),
            "menu" => Ok(Self::Menu),
            "minus" => Ok(Self::Minus),
            "monitor" => Ok(Self::Monitor),
            "moon" => Ok(Self::Moon),
            "package" => Ok(Self::Package),
            "palette" => Ok(Self::Palette),
            "panel-left-close" => Ok(Self::PanelLeftClose),
            "panel-left-open" => Ok(Self::PanelLeftOpen),
            "pencil" => Ok(Self::Pencil),
            "plus" => Ok(Self::Plus),
            "qr-code" => Ok(Self::QrCode),
            "receipt" => Ok(Self::Receipt),
            "redo-2" => Ok(Self::Redo2),
            "refresh-cw" => Ok(Self::RefreshCw),
            "remove-formatting" => Ok(Self::RemoveFormatting),
            "save" => Ok(Self::Save),
            "scroll-text" => Ok(Self::ScrollText),
            "search" => Ok(Self::Search),
            "settings" => Ok(Self::Settings),
            "shield" => Ok(Self::Shield),
            "shield-check" => Ok(Self::ShieldCheck),
            "shield-off" => Ok(Self::ShieldOff),
            "shopping-cart" => Ok(Self::ShoppingCart),
            "sliders-horizontal" => Ok(Self::SlidersHorizontal),
            "smartphone" => Ok(Self::Smartphone),
            "strikethrough" => Ok(Self::Strikethrough),
            "sun" => Ok(Self::Sun),
            "table" => Ok(Self::Table),
            "text-quote" => Ok(Self::TextQuote),
            "trash-2" => Ok(Self::Trash2),
            "triangle-alert" => Ok(Self::TriangleAlert),
            "truck" => Ok(Self::Truck),
            "underline" => Ok(Self::Underline),
            "undo-2" => Ok(Self::Undo2),
            "upload" => Ok(Self::Upload),
            "user" => Ok(Self::User),
            "user-plus" => Ok(Self::UserPlus),
            "users" => Ok(Self::Users),
            "warehouse" => Ok(Self::Warehouse),
            "x" => Ok(Self::X),
            _ => Err(()),
        }
    }
}
