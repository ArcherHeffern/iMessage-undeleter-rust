/*!
 Represents CLI options and validation logic.
*/

use std::path::PathBuf;

use clap::{Arg, ArgAction, ArgMatches, Command, crate_version};

use imessage_database::{
    tables::{attachment::DEFAULT_ATTACHMENT_ROOT, table::DEFAULT_PATH_IOS},
    util::{
        dirs::{default_db_path, home},
        platform::Platform,
        query_context::QueryContext,
    },
};

use crate::app::{
    compatibility::attachment_manager::{AttachmentManager, AttachmentManagerMode},
    error::RuntimeError,
};

/// Default export directory name
pub const DEFAULT_OUTPUT_DIR: &str = "undeleted_messages";

// CLI Arg Names
pub const OPTION_DB_PATH: &str = "db-path";
pub const OPTION_ATTACHMENT_ROOT: &str = "attachment-root";
pub const OPTION_EXPORT_PATH: &str = "export-path";
pub const OPTION_CHECK_LAST_N_MESSAGES: &str = "check-last-n";
pub const OPTION_DISABLE_LAZY_LOADING: &str = "no-lazy";
pub const OPTION_CUSTOM_NAME: &str = "custom-name";
pub const OPTION_PLATFORM: &str = "platform";
pub const OPTION_USE_CALLER_ID: &str = "use-caller-id";
pub const OPTION_CONVERSATION_FILTER: &str = "conversation-filter";
pub const OPTION_CLEARTEXT_PASSWORD: &str = "cleartext-password";

// Other CLI Text
pub const SUPPORTED_PLATFORMS: &str = "macOS, iOS";
pub const ABOUT: &str = concat!(
    "The `imessage-undeleter` binary watches iMessage conversations for deleted messages.\n",
    "It can also run diagnostics\n",
    "to find problems with the iMessage database."
);

#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    /// Path to database file
    pub db_path: PathBuf,
    /// Custom path to attachments
    pub attachment_root: Option<String>,
    /// The attachment manager type used to copy files
    pub attachment_manager: AttachmentManager,
    /// Where the app will save exported data
    pub export_path: PathBuf,
    /// Query context describing SQL query filters
    pub query_context: QueryContext,
    /// If true, do not include `loading="lazy"` in HTML exports
    pub no_lazy: bool,
    /// Custom name for database owner in output
    pub custom_name: Option<String>,
    /// If true, use the database owner's caller ID instead of "Me"
    pub use_caller_id: bool,
    /// The database source's platform
    pub platform: Platform,
    /// An optional filter for conversation participants
    pub conversation_filter: Option<String>,
    /// An optional password for encrypted backups
    pub cleartext_password: Option<String>,
}

impl Options {
    pub fn from_args(args: &ArgMatches) -> Result<Self, RuntimeError> {
        let user_path: Option<&String> = args.get_one(OPTION_DB_PATH);
        let attachment_root: Option<&String> = args.get_one(OPTION_ATTACHMENT_ROOT);
        let user_export_path: Option<&String> = args.get_one(OPTION_EXPORT_PATH);
        let check_last_n_messages_string: Option<&String> = args.get_one(OPTION_CHECK_LAST_N_MESSAGES);
        let no_lazy = args.get_flag(OPTION_DISABLE_LAZY_LOADING);
        let custom_name: Option<&String> = args.get_one(OPTION_CUSTOM_NAME);
        let use_caller_id = args.get_flag(OPTION_USE_CALLER_ID);
        let platform_type: Option<&String> = args.get_one(OPTION_PLATFORM);
        let conversation_filter: Option<&String> = args.get_one(OPTION_CONVERSATION_FILTER);
        let cleartext_password: Option<&String> = args.get_one(OPTION_CLEARTEXT_PASSWORD);

        let check_last_n_messages: Option<i32> = check_last_n_messages_string.map(|s| s.parse::<i32>().ok()).flatten();

        // Prevent custom_name vs. use_caller_id collision
        if custom_name.is_some() && use_caller_id {
            return Err(RuntimeError::InvalidOptions(format!(
                "--{OPTION_CUSTOM_NAME} is enabled; --{OPTION_USE_CALLER_ID} is disallowed"
            )));
        }

        // Build query context
        let mut query_context = QueryContext::default();
        if let Some(limit) = check_last_n_messages {
            query_context.set_limit(limit.clone());
        }

        // We have to allocate a PathBuf here because it can be created from data owned by this function in the default state
        let db_path = match user_path {
            Some(path) => PathBuf::from(path),
            None => default_db_path(),
        };

        // Build the Platform
        let platform = match platform_type {
            Some(platform_str) => {
                Platform::from_cli(platform_str).ok_or(RuntimeError::InvalidOptions(format!(
                    "{platform_str} is not a valid platform! Must be one of <{SUPPORTED_PLATFORMS}>"
                )))?
            }
            None => Platform::determine(&db_path)?,
        };

        // Prevent cleartext_password from being set if the source is not an iOS backup
        if cleartext_password.is_some() && !matches!(platform, Platform::iOS) {
            return Err(RuntimeError::InvalidOptions(format!(
                "--{OPTION_CLEARTEXT_PASSWORD} is enabled; it can only be used with iOS backups."
            )));
        }

        // Validate that the custom attachment root exists, if provided
        if let Some(path) = attachment_root {
            let custom_attachment_path = PathBuf::from(path);
            if !custom_attachment_path.exists() {
                return Err(RuntimeError::InvalidOptions(format!(
                    "Supplied {OPTION_ATTACHMENT_ROOT} `{path}` does not exist!"
                )));
            }
        }

        // Warn the user that custom attachment roots have no effect on iOS backups
        if attachment_root.is_some() && platform == Platform::iOS {
            eprintln!(
                "Option {OPTION_ATTACHMENT_ROOT} is enabled, but the platform is {}, so the root will have no effect!",
                Platform::iOS
            );
        }

        // Determine the attachment manager mode
        let attachment_manager_mode = AttachmentManagerMode::default();

        // Validate the provided export path
        let export_path = PathBuf::from(user_export_path.unwrap_or(&format!("./{DEFAULT_OUTPUT_DIR}")));

        Ok(Options {
            db_path,
            attachment_root: attachment_root.cloned(),
            attachment_manager: AttachmentManager::from(attachment_manager_mode),
            export_path,
            query_context,
            no_lazy,
            custom_name: custom_name.cloned(),
            use_caller_id,
            platform,
            conversation_filter: conversation_filter.cloned(),
            cleartext_password: cleartext_password.cloned(),
        })
    }

    /// Generate a path to the database based on the currently selected platform
    pub fn get_db_path(&self) -> PathBuf {
        match self.platform {
            Platform::iOS => self.db_path.join(DEFAULT_PATH_IOS),
            Platform::macOS => self.db_path.clone(),
        }
    }
}

/// Ensure export path is empty or does not contain files of the existing export type
///
/// We have to allocate a `PathBuf` here because it can be created from data owned by this function in the default state

/// Build the command line argument parser
fn get_command() -> Command {
    Command::new("iMessage Exporter")
        .version(crate_version!())
        .about(ABOUT)
        .arg_required_else_help(true)
        .arg(
            Arg::new(OPTION_CHECK_LAST_N_MESSAGES)
                .short('n')
                .long(OPTION_CHECK_LAST_N_MESSAGES)
                .help("Only Check last n messages")
                .display_order(2)
                .value_name("10"),
        )
        .arg(
            Arg::new(OPTION_DB_PATH)
                .short('p')
                .long(OPTION_DB_PATH)
                .help(format!("Specify an optional custom path for the iMessage database location\nFor macOS, specify a path to a `chat.db` file\nFor iOS, specify a path to the root of a device backup directory\nIf the iOS backup is encrypted, --{OPTION_CLEARTEXT_PASSWORD} must be passed\nIf omitted, the default directory is {}\n", default_db_path().display()))
                .display_order(3)
                .value_name("path/to/source"),
        )
        .arg(
            Arg::new(OPTION_ATTACHMENT_ROOT)
                .short('r')
                .long(OPTION_ATTACHMENT_ROOT)
                .help(format!("Specify an optional custom path to look for attachments in (macOS only)\nOnly use this if attachments are stored separately from the database's default location\nThe default location is {}\n", DEFAULT_ATTACHMENT_ROOT.replacen('~', &home(), 1)))
                .display_order(4)
                .value_name("path/to/attachments"),
        )
        .arg(
            Arg::new(OPTION_PLATFORM)
            .short('a')
            .long(OPTION_PLATFORM)
            .help("Specify the platform the database was created on\nIf omitted, the platform type is determined automatically\n")
            .display_order(5)
            .value_name(SUPPORTED_PLATFORMS),
        )
        .arg(
            Arg::new(OPTION_EXPORT_PATH)
                .short('o')
                .long(OPTION_EXPORT_PATH)
                .help(format!("Specify an optional custom directory for outputting exported data\nIf omitted, the default directory is {}/{DEFAULT_OUTPUT_DIR}\n", home()))
                .display_order(6)
                .value_name("path/to/save/files"),
        )
        .arg(
            Arg::new(OPTION_DISABLE_LAZY_LOADING)
                .short('l')
                .long(OPTION_DISABLE_LAZY_LOADING)
                .help("Do not include `loading=\"lazy\"` in HTML export `img` tags\nThis will make pages load slower but PDF generation work\n")
                .action(ArgAction::SetTrue)
                .display_order(9),
        )
        .arg(
            Arg::new(OPTION_CUSTOM_NAME)
                .short('m')
                .long(OPTION_CUSTOM_NAME)
                .help(format!("Specify an optional custom name for the database owner's messages in exports\nConflicts with --{OPTION_USE_CALLER_ID}\n"))
                .display_order(10)
        )
        .arg(
            Arg::new(OPTION_USE_CALLER_ID)
                .short('i')
                .long(OPTION_USE_CALLER_ID)
                .help(format!("Use the database owner's caller ID in exports instead of \"Me\"\nConflicts with --{OPTION_CUSTOM_NAME}\n"))
                .action(ArgAction::SetTrue)
                .display_order(11)
        )
        .arg(
            Arg::new(OPTION_CONVERSATION_FILTER)
                .short('t')
                .long(OPTION_CONVERSATION_FILTER)
                .help("Filter exported conversations by contact numbers or emails\nTo provide multiple filter criteria, use a comma-separated string\nAll conversations with the specified participants are exported, including group conversations\nExample: `-t steve@apple.com,5558675309`\n")
                .display_order(13)
                .value_name("filter"),
        )
        .arg(
            Arg::new(OPTION_CLEARTEXT_PASSWORD)
                .short('x')
                .long(OPTION_CLEARTEXT_PASSWORD)
                .help("Optional password for encrypted iOS backups\nThis is only used when the source is an encrypted iOS backup directory\n")
                .display_order(14)
                .value_name("password"),
        )
}

/// Parse arguments from the command line
pub fn from_command_line() -> ArgMatches {
    get_command().get_matches()
}
