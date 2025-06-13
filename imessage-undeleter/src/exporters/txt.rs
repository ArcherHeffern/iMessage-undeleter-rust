use std::{
    collections::{
        HashMap,
        hash_map::Entry::{Occupied, Vacant},
    },
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
};

use crate::{
    app::{
        compatibility::attachment_manager::AttachmentManagerMode, error::RuntimeError,
        progress::ExportProgress, runtime::Config,
    },
    exporters::exporter::{ATTACHMENT_NO_FILENAME, BalloonFormatter, Exporter, Writer},
};

use imessage_database::{
    error::{plist::PlistParseError, table::TableError},
    message_types::{
        app::AppMessage,
        app_store::AppStoreMessage,
        collaboration::CollaborationMessage,
        digital_touch::{self, DigitalTouch},
        edited::{EditStatus, EditedMessage},
        expressives::{BubbleEffect, Expressive, ScreenEffect},
        handwriting::HandwrittenMessage,
        music::MusicMessage,
        placemark::PlacemarkMessage,
        sticker::StickerSource,
        url::URLMessage,
        variants::{
            Announcement, BalloonProvider, CustomBalloon, Tapback, TapbackAction, URLOverride,
            Variant,
        },
    },
    tables::{
        attachment::{Attachment, MediaType},
        messages::{
            Message,
            models::{AttachmentMeta, BubbleComponent, GroupAction, TextAttributes},
        },
        table::{AttributedBody, FITNESS_RECEIVER, ME, ORPHANED, Table, YOU},
    },
    util::{
        dates::{TIMESTAMP_FACTOR, format, get_local_time, readable_diff},
        plist::parse_ns_keyed_archiver,
    },
};

pub struct TXT<'a> {
    /// Data that is setup from the application's runtime
    pub config: &'a Config,
    /// Handles to files we want to write messages to
    /// Map of resolved chatroom file location to a buffered writer
    pub files: HashMap<String, BufWriter<File>>,
    /// Writer instance for orphaned messages
    pub orphaned: BufWriter<File>,
    /// Progress Bar model for alerting the user about current export state
    pb: ExportProgress,
}

impl<'a> Exporter<'a> for TXT<'a> {
    fn new(config: &'a Config) -> Result<Self, RuntimeError> {
        let mut orphaned = config.options.export_path.clone();
        orphaned.push(ORPHANED);
        orphaned.set_extension("txt");

        let file = File::options().append(true).create(true).open(&orphaned)?;

        Ok(TXT {
            config,
            files: HashMap::new(),
            orphaned: BufWriter::new(file),
            pb: ExportProgress::new(),
        })
    }

    fn iter_messages(&mut self) -> Result<(), RuntimeError> {
        // Tell the user what we are doing
        eprintln!(
            "Exporting to {} as txt...",
            self.config.options.export_path.display()
        );

        // Keep track of current message ROWID
        let mut current_message_row = -1;

        // Set up progress bar
        let mut current_message = 0;
        let total_messages =
            Message::get_count(self.config.db(), &self.config.options.query_context)?;
        self.pb.start(total_messages);

        let mut statement =
            Message::stream_rows(self.config.db(), &self.config.options.query_context)?;

        let messages = statement
            .query_map([], |row| Ok(Message::from_row(row)))
            .map_err(|err| RuntimeError::DatabaseError(TableError::Messages(err)))?;

        for message in messages {
            let mut msg = Message::extract(message)?;

            // Early escape if we try and render the same message GUID twice
            // See https://github.com/ReagentX/imessage-exporter/issues/135 for rationale
            if msg.rowid == current_message_row {
                current_message += 1;
                continue;
            }
            current_message_row = msg.rowid;

            // Generate the text of the message
            let _ = msg.generate_text(self.config.db());

            // Render the announcement in-line
            if msg.is_announcement() {
                let announcement = self.format_announcement(&msg);
                TXT::write_to_file(self.get_or_create_file(&msg)?, &announcement)?;
            }
            // Message replies and tapbacks are rendered in context, so no need to render them separately
            else if !msg.is_tapback() {
                let message = self.format_message(&msg, 0)?;
                TXT::write_to_file(self.get_or_create_file(&msg)?, &message)?;
            }
            current_message += 1;
            if current_message % 99 == 0 {
                self.pb.set_position(current_message);
            }
        }
        self.pb.finish();
        Ok(())
    }

    /// Create a file for the given chat, caching it so we don't need to build it later
    fn get_or_create_file(
        &mut self,
        message: &Message,
    ) -> Result<&mut BufWriter<File>, RuntimeError> {
        match self.config.conversation(message) {
            Some((chatroom, _)) => {
                let filename = self.config.filename(chatroom);
                match self.files.entry(filename) {
                    Occupied(entry) => Ok(entry.into_mut()),
                    Vacant(entry) => {
                        let mut path = self.config.options.export_path.clone();
                        path.push(self.config.filename(chatroom));
                        path.set_extension("txt");

                        let file = File::options().append(true).create(true).open(&path)?;

                        Ok(entry.insert(BufWriter::new(file)))
                    }
                }
            }
            None => Ok(&mut self.orphaned),
        }
    }
}

impl<'a> Writer<'a> for TXT<'a> {
    fn format_message(&self, message: &Message, indent_size: usize) -> Result<String, TableError> {
        let indent = String::from_iter((0..indent_size).map(|_| " "));
        // Data we want to write to a file
        let mut formatted_message = String::new();

        // Add message date
        self.add_line(&mut formatted_message, &self.get_time(message), &indent);

        // Add message sender
        self.add_line(
            &mut formatted_message,
            self.config.who(
                message.handle_id,
                message.is_from_me(),
                &message.destination_caller_id,
            ),
            &indent,
        );

        // If message was deleted, annotate it
        if message.is_deleted() {
            self.add_line(
                &mut formatted_message,
                "This message was deleted from the conversation!",
                &indent,
            );
        }

        // Useful message metadata
        let message_parts = message.body();
        let mut attachments = Attachment::from_message(self.config.db(), message)?;
        let mut replies = message.get_replies(self.config.db())?;

        // Index of where we are in the attachment Vector
        let mut attachment_index: usize = 0;

        // Render subject
        if let Some(subject) = &message.subject {
            self.add_line(&mut formatted_message, subject, &indent);
        }

        // Handle SharePlay
        if message.is_shareplay() {
            self.add_line(&mut formatted_message, self.format_shareplay(), &indent);
        }

        // Handle Shared Location
        if message.started_sharing_location() || message.stopped_sharing_location() {
            self.add_line(
                &mut formatted_message,
                self.format_shared_location(message),
                &indent,
            );
        }

        // Generate the message body from it's components
        for (idx, message_part) in message_parts.iter().enumerate() {
            match message_part {
                // Fitness messages have a prefix that we need to replace with the opposite if who sent the message
                BubbleComponent::Text(text_attrs) => {
                    if let Some(text) = &message.text {
                        // Render edited message content, if applicable
                        if message.is_part_edited(idx) {
                            if let Some(edited_parts) = &message.edited_parts {
                                if let Some(edited) =
                                    self.format_edited(message, edited_parts, idx, &indent)
                                {
                                    self.add_line(&mut formatted_message, &edited, &indent);
                                }
                            }
                        } else {
                            let mut formatted_text = self.format_attributes(text, text_attrs);

                            // If we failed to parse any text above, use the original text
                            if formatted_text.is_empty() {
                                formatted_text.push_str(text);
                            }

                            if formatted_text.starts_with(FITNESS_RECEIVER) {
                                self.add_line(
                                    &mut formatted_message,
                                    &formatted_text.replace(FITNESS_RECEIVER, YOU),
                                    &indent,
                                );
                            } else {
                                self.add_line(&mut formatted_message, &formatted_text, &indent);
                            }
                        }
                    }
                }
                BubbleComponent::Attachment(metadata) => {
                    match attachments.get_mut(attachment_index) {
                        Some(attachment) => {
                            if attachment.is_sticker {
                                let result = self.format_sticker(attachment, message);
                                self.add_line(&mut formatted_message, &result, &indent);
                            } else {
                                match self.format_attachment(attachment, message, metadata) {
                                    Ok(result) => {
                                        attachment_index += 1;
                                        self.add_line(&mut formatted_message, &result, &indent);
                                    }
                                    Err(result) => {
                                        self.add_line(&mut formatted_message, result, &indent);
                                    }
                                }
                            }
                        }
                        // Attachment does not exist in attachments table
                        None => {
                            self.add_line(&mut formatted_message, "Attachment missing!", &indent);
                        }
                    }
                }
                BubbleComponent::App => match self.format_app(message, &mut attachments, &indent) {
                    // We use an empty indent here because `format_app` handles building the entire message
                    Ok(ok_bubble) => self.add_line(&mut formatted_message, &ok_bubble, &indent),
                    Err(why) => self.add_line(
                        &mut formatted_message,
                        &format!("Unable to format app message: {why}"),
                        &indent,
                    ),
                },
                BubbleComponent::Retracted => {
                    if let Some(edited_parts) = &message.edited_parts {
                        if let Some(edited) =
                            self.format_edited(message, edited_parts, idx, &indent)
                        {
                            self.add_line(&mut formatted_message, &edited, &indent);
                        }
                    }
                }
            }

            // Handle expressives
            if message.expressive_send_style_id.is_some() {
                self.add_line(
                    &mut formatted_message,
                    self.format_expressive(message),
                    &indent,
                );
            }

            // Handle Tapbacks
            if let Some(tapbacks_map) = self.config.tapbacks.get(&message.guid) {
                if let Some(tapbacks) = tapbacks_map.get(&idx) {
                    let mut formatted_tapbacks = String::new();
                    tapbacks
                        .iter()
                        .try_for_each(|tapbacks| -> Result<(), TableError> {
                            let formatted = self.format_tapback(tapbacks)?;
                            if !formatted.is_empty() {
                                self.add_line(
                                    &mut formatted_tapbacks,
                                    &self.format_tapback(tapbacks)?,
                                    &indent,
                                );
                            }
                            Ok(())
                        })?;

                    if !formatted_tapbacks.is_empty() {
                        self.add_line(&mut formatted_message, "Tapbacks:", &indent);
                        self.add_line(&mut formatted_message, &formatted_tapbacks, &indent);
                    }
                }
            }

            // Handle Replies
            if let Some(replies) = replies.get_mut(&idx) {
                replies
                    .iter_mut()
                    .try_for_each(|reply| -> Result<(), TableError> {
                        let _ = reply.generate_text(self.config.db());
                        if !reply.is_tapback() {
                            self.add_line(
                                &mut formatted_message,
                                &self.format_message(reply, 4)?,
                                &indent,
                            );
                        }
                        Ok(())
                    })?;
            }
        }

        // Add a note if the message is a reply
        if message.is_reply() && indent.is_empty() {
            self.add_line(
                &mut formatted_message,
                "This message responded to an earlier message.",
                &indent,
            );
        }

        if indent.is_empty() {
            // Add a newline for top-level messages
            formatted_message.push('\n');
        }

        Ok(formatted_message)
    }

    fn format_attachment(
        &self,
        attachment: &'a mut Attachment,
        message: &Message,
        metadata: &AttachmentMeta,
    ) -> Result<String, &'a str> {
        // When encoding videos, alert the user that the time estimate may be inaccurate
        let will_encode = matches!(attachment.mime_type(), MediaType::Video(_))
            && matches!(
                self.config.options.attachment_manager.mode,
                AttachmentManagerMode::Full
            );

        if will_encode {
            self.pb
                .set_busy_style("Encoding video, estimates paused...".to_string());
        }

        // Copy the file, if requested
        self.config
            .options
            .attachment_manager
            .handle_attachment(message, attachment, self.config)
            .ok_or(attachment.filename().ok_or(ATTACHMENT_NO_FILENAME)?)?;

        if will_encode {
            self.pb.set_default_style();
        }

        // Append the transcription if one is provided
        if let Some(transcription) = metadata.transcription {
            return Ok(format!(
                "{}\nTranscription: {transcription}",
                self.config.message_attachment_path(attachment)
            ));
        }

        // Build a relative filepath from the fully qualified one on the `Attachment`
        Ok(self.config.message_attachment_path(attachment))
    }

    fn format_sticker(&self, sticker: &'a mut Attachment, message: &Message) -> String {
        let who = self.config.who(
            message.handle_id,
            message.is_from_me(),
            &message.destination_caller_id,
        );
        match self.format_attachment(sticker, message, &AttachmentMeta::default()) {
            Ok(path_to_sticker) => {
                let mut out_s = format!("Sticker from {who}: {path_to_sticker}");

                // Determine the source of the sticker
                if let Some(sticker_source) = sticker.get_sticker_source(self.config.db()) {
                    match sticker_source {
                        StickerSource::Genmoji => {
                            // Add sticker prompt
                            if let Some(prompt) = &sticker.emoji_description {
                                out_s = format!("{out_s} (Genmoji prompt: {prompt})");
                            }
                        }
                        StickerSource::Memoji => out_s.push_str(" (App: Memoji)"),
                        StickerSource::UserGenerated => {
                            // Add sticker effect
                            if let Ok(Some(sticker_effect)) = sticker.get_sticker_effect(
                                &self.config.options.platform,
                                &self.config.options.db_path,
                                self.config.options.attachment_root.as_deref(),
                            ) {
                                out_s = format!("{sticker_effect} {out_s}");
                            }
                        }
                        StickerSource::App(bundle_id) => {
                            // Add the application name used to generate/send the sticker
                            let app_name = sticker
                                .get_sticker_source_application_name(self.config.db())
                                .unwrap_or(bundle_id);
                            out_s.push_str(&format!(" (App: {app_name})"));
                        }
                    }
                }

                out_s
            }
            Err(path) => format!("Sticker from {who}: {path}"),
        }
    }

    fn format_app(
        &self,
        message: &'a Message,
        attachments: &mut Vec<Attachment>,
        indent: &str,
    ) -> Result<String, PlistParseError> {
        if let Variant::App(balloon) = message.variant() {
            let mut app_bubble = String::new();

            // Handwritten messages use a different payload type, so check that first
            if message.is_handwriting() {
                if let Some(payload) = message.raw_payload_data(self.config.db()) {
                    return match HandwrittenMessage::from_payload(&payload) {
                        Ok(bubble) => Ok(self.format_handwriting(message, &bubble, indent)),
                        Err(why) => Err(PlistParseError::HandwritingError(why)),
                    };
                }
            }

            if message.is_digital_touch() {
                if let Some(payload) = message.raw_payload_data(self.config.db()) {
                    return match digital_touch::from_payload(&payload) {
                        Some(bubble) => Ok(self.format_digital_touch(message, &bubble, indent)),
                        None => Err(PlistParseError::DigitalTouchError),
                    };
                }
            }

            if let Some(payload) = message.payload_data(self.config.db()) {
                // Handle URL messages separately since they are a special case
                let parsed = parse_ns_keyed_archiver(&payload)?;
                let res = if message.is_url() {
                    let bubble = URLMessage::get_url_message_override(&parsed)?;
                    match bubble {
                        URLOverride::Normal(balloon) => self.format_url(message, &balloon, indent),
                        URLOverride::AppleMusic(balloon) => self.format_music(&balloon, indent),
                        URLOverride::Collaboration(balloon) => {
                            self.format_collaboration(&balloon, indent)
                        }
                        URLOverride::AppStore(balloon) => self.format_app_store(&balloon, indent),
                        URLOverride::SharedPlacemark(balloon) => {
                            self.format_placemark(&balloon, indent)
                        }
                    }
                // Handwriting uses a different payload type than the rest of the branches
                } else {
                    // Handle the app case
                    match AppMessage::from_map(&parsed) {
                        Ok(bubble) => match balloon {
                            CustomBalloon::Application(bundle_id) => {
                                self.format_generic_app(&bubble, bundle_id, attachments, indent)
                            }
                            CustomBalloon::ApplePay => self.format_apple_pay(&bubble, indent),
                            CustomBalloon::Fitness => self.format_fitness(&bubble, indent),
                            CustomBalloon::Slideshow => self.format_slideshow(&bubble, indent),
                            CustomBalloon::CheckIn => self.format_check_in(&bubble, indent),
                            CustomBalloon::FindMy => self.format_find_my(&bubble, indent),
                            CustomBalloon::Handwriting => unreachable!(),
                            CustomBalloon::DigitalTouch => unreachable!(),
                            CustomBalloon::URL => unreachable!(),
                        },
                        Err(why) => return Err(why),
                    }
                };
                app_bubble.push_str(&res);
            } else {
                // Sometimes, URL messages are missing their payloads
                if message.is_url() {
                    if let Some(text) = &message.text {
                        return Ok(text.to_string());
                    }
                }
                return Err(PlistParseError::NoPayload);
            }
            Ok(app_bubble)
        } else {
            Err(PlistParseError::WrongMessageType)
        }
    }

    fn format_tapback(&self, msg: &Message) -> Result<String, TableError> {
        match msg.variant() {
            Variant::Tapback(_, action, tapback) => {
                if let TapbackAction::Removed = action {
                    return Ok(String::new());
                }

                match tapback {
                    Tapback::Sticker => {
                        let mut paths = Attachment::from_message(self.config.db(), msg)?;
                        let who = self.config.who(
                            msg.handle_id,
                            msg.is_from_me(),
                            &msg.destination_caller_id,
                        );
                        // Sticker messages have only one attachment, the sticker image
                        Ok(if let Some(sticker) = paths.get_mut(0) {
                            format!("{} from {who}", self.format_sticker(sticker, msg))
                        } else {
                            format!("Sticker from {who} not found!")
                        })
                    }
                    _ => Ok(format!(
                        "{} by {}",
                        tapback,
                        self.config.who(
                            msg.handle_id,
                            msg.is_from_me(),
                            &msg.destination_caller_id
                        ),
                    )),
                }
            }
            _ => unreachable!(),
        }
    }

    fn format_expressive(&self, msg: &'a Message) -> &'a str {
        match msg.get_expressive() {
            Expressive::Screen(effect) => match effect {
                ScreenEffect::Confetti => "Sent with Confetti",
                ScreenEffect::Echo => "Sent with Echo",
                ScreenEffect::Fireworks => "Sent with Fireworks",
                ScreenEffect::Balloons => "Sent with Balloons",
                ScreenEffect::Heart => "Sent with Heart",
                ScreenEffect::Lasers => "Sent with Lasers",
                ScreenEffect::ShootingStar => "Sent with Shooting Star",
                ScreenEffect::Sparkles => "Sent with Sparkles",
                ScreenEffect::Spotlight => "Sent with Spotlight",
            },
            Expressive::Bubble(effect) => match effect {
                BubbleEffect::Slam => "Sent with Slam",
                BubbleEffect::Loud => "Sent with Loud",
                BubbleEffect::Gentle => "Sent with Gentle",
                BubbleEffect::InvisibleInk => "Sent with Invisible Ink",
            },
            Expressive::Unknown(effect) => effect,
            Expressive::None => "",
        }
    }

    fn format_announcement(&self, msg: &'a Message) -> String {
        let mut who = self
            .config
            .who(msg.handle_id, msg.is_from_me(), &msg.destination_caller_id);
        // Rename yourself so we render the proper grammar here
        if who == ME {
            who = self.config.options.custom_name.as_deref().unwrap_or(YOU);
        }

        let timestamp = format(&msg.date(&self.config.offset));

        match msg.get_announcement() {
            Some(announcement) => {
                let action_text = match announcement {
                    Announcement::GroupAction(action) => match action {
                        GroupAction::ParticipantAdded(person)
                        | GroupAction::ParticipantRemoved(person) => {
                            let resolved_person =
                                self.config
                                    .who(Some(person), false, &msg.destination_caller_id);
                            let action_word = if matches!(action, GroupAction::ParticipantAdded(_))
                            {
                                "added"
                            } else {
                                "removed"
                            };
                            format!(
                                "{action_word} {resolved_person} {} the conversation.",
                                if matches!(action, GroupAction::ParticipantAdded(_)) {
                                    "to"
                                } else {
                                    "from"
                                }
                            )
                        }
                        GroupAction::NameChange(name) => {
                            format!("renamed the conversation to {name}")
                        }
                        GroupAction::ParticipantLeft => "left the conversation.".to_string(),
                        GroupAction::GroupIconChanged => "changed the group photo.".to_string(),
                        GroupAction::GroupIconRemoved => "removed the group photo.".to_string(),
                    },
                    Announcement::AudioMessageKept => "kept an audio message.".to_string(),
                    Announcement::FullyUnsent => "unsent a message!".to_string(),
                    Announcement::Unknown(num) => format!("performed unknown action {num}"),
                };
                format!("{timestamp} {who} {action_text}\n\n")
            }
            None => String::from("Unable to format announcement!\n\n"),
        }
    }

    fn format_shareplay(&self) -> &'static str {
        "SharePlay Message\nEnded"
    }

    fn format_shared_location(&self, msg: &'a Message) -> &'static str {
        // Handle Shared Location
        if msg.started_sharing_location() {
            return "Started sharing location!";
        } else if msg.stopped_sharing_location() {
            return "Stopped sharing location!";
        }
        "Shared location!"
    }

    fn format_edited(
        &self,
        msg: &'a Message,
        edited_message: &'a EditedMessage,
        message_part_idx: usize,
        indent: &str,
    ) -> Option<String> {
        if let Some(edited_message_part) = edited_message.part(message_part_idx) {
            let mut out_s = String::new();
            let mut previous_timestamp: Option<&i64> = None;

            match edited_message_part.status {
                EditStatus::Edited => {
                    for event in &edited_message_part.edit_history {
                        match previous_timestamp {
                            // Original message get an absolute timestamp
                            None => {
                                let parsed_timestamp =
                                    format(&get_local_time(&event.date, &self.config.offset));
                                out_s.push_str(&parsed_timestamp);
                                out_s.push(' ');
                            }
                            // Subsequent edits get a relative timestamp
                            Some(prev_timestamp) => {
                                let end = get_local_time(&event.date, &self.config.offset);
                                let start = get_local_time(prev_timestamp, &self.config.offset);
                                if let Some(diff) = readable_diff(start, end) {
                                    out_s.push_str(indent);
                                    out_s.push_str("Edited ");
                                    out_s.push_str(&diff);
                                    out_s.push_str(" later: ");
                                }
                            }
                        }

                        // Update the previous timestamp for the next loop
                        previous_timestamp = Some(&event.date);

                        // Render the message text
                        if let Some(text) = &event.text {
                            self.add_line(&mut out_s, text, indent);
                        }
                    }
                }
                EditStatus::Unsent => {
                    let who = if msg.is_from_me() {
                        self.config.options.custom_name.as_deref().unwrap_or(YOU)
                    } else {
                        "They"
                    };

                    if let Some(diff) = readable_diff(
                        msg.date(&self.config.offset),
                        msg.date_edited(&self.config.offset),
                    ) {
                        out_s.push_str(who);
                        out_s.push_str(" unsent this message part ");
                        out_s.push_str(&diff);
                        out_s.push_str(" after sending!");
                    } else {
                        out_s.push_str(who);
                        out_s.push_str(" unsent this message part!");
                    }
                }
                EditStatus::Original => {
                    return None;
                }
            }

            return Some(out_s);
        }
        None
    }

    fn format_attributes(&'a self, text: &'a str, effects: &'a [TextAttributes]) -> String {
        let mut formatted_text: String = String::with_capacity(text.len());
        for effect in effects {
            if let Some(message_content) = text.get(effect.start..effect.end) {
                // There isn't really a way to represent formatted text in a plain text export
                formatted_text.push_str(message_content);
            }
        }
        formatted_text
    }

    fn write_to_file(file: &mut BufWriter<File>, text: &str) -> Result<(), RuntimeError> {
        file.write_all(text.as_bytes())
            .map_err(RuntimeError::DiskError)
    }
}

impl<'a> BalloonFormatter<&'a str> for TXT<'a> {
    fn format_url(&self, msg: &Message, balloon: &URLMessage, indent: &str) -> String {
        let mut out_s = String::new();

        if let Some(url) = balloon.get_url() {
            self.add_line(&mut out_s, url, indent);
        } else if let Some(text) = &msg.text {
            self.add_line(&mut out_s, text, indent);
        }

        if let Some(title) = balloon.title {
            self.add_line(&mut out_s, title, indent);
        }

        if let Some(summary) = balloon.summary {
            self.add_line(&mut out_s, summary, indent);
        }

        // We want to keep the newlines between blocks, but the last one should be removed
        out_s.strip_suffix('\n').unwrap_or(&out_s).to_string()
    }

    fn format_music(&self, balloon: &MusicMessage, indent: &str) -> String {
        let mut out_s = String::new();

        if let Some(lyrics) = &balloon.lyrics {
            self.add_line(&mut out_s, "Lyrics:", indent);
            for line in lyrics {
                self.add_line(&mut out_s, line, indent);
            }
            self.add_line(&mut out_s, "\n", indent);
        }

        if let Some(track_name) = balloon.track_name {
            self.add_line(&mut out_s, track_name, indent);
        }

        if let Some(album) = balloon.album {
            self.add_line(&mut out_s, album, indent);
        }

        if let Some(artist) = balloon.artist {
            self.add_line(&mut out_s, artist, indent);
        }

        if let Some(url) = balloon.url {
            self.add_line(&mut out_s, url, indent);
        }

        out_s
    }

    fn format_collaboration(&self, balloon: &CollaborationMessage, indent: &str) -> String {
        let mut out_s = String::from(indent);

        if let Some(name) = balloon.app_name {
            out_s.push_str(name);
        } else if let Some(bundle_id) = balloon.bundle_id {
            out_s.push_str(bundle_id);
        }

        if !out_s.is_empty() {
            out_s.push_str(" message:\n");
        }

        if let Some(title) = balloon.title {
            self.add_line(&mut out_s, title, indent);
        }

        if let Some(url) = balloon.get_url() {
            self.add_line(&mut out_s, url, indent);
        }

        // We want to keep the newlines between blocks, but the last one should be removed
        out_s.strip_suffix('\n').unwrap_or(&out_s).to_string()
    }

    fn format_app_store(&self, balloon: &AppStoreMessage, indent: &'a str) -> String {
        let mut out_s = String::from(indent);

        if let Some(name) = balloon.app_name {
            self.add_line(&mut out_s, name, indent);
        }

        if let Some(description) = balloon.description {
            self.add_line(&mut out_s, description, indent);
        }

        if let Some(platform) = balloon.platform {
            self.add_line(&mut out_s, platform, indent);
        }

        if let Some(genre) = balloon.genre {
            self.add_line(&mut out_s, genre, indent);
        }

        if let Some(url) = balloon.url {
            self.add_line(&mut out_s, url, indent);
        }

        // We want to keep the newlines between blocks, but the last one should be removed
        out_s.strip_suffix('\n').unwrap_or(&out_s).to_string()
    }

    fn format_placemark(&self, balloon: &PlacemarkMessage, indent: &'a str) -> String {
        let mut out_s = String::from(indent);

        if let Some(name) = balloon.place_name {
            self.add_line(&mut out_s, name, indent);
        }

        if let Some(url) = balloon.get_url() {
            self.add_line(&mut out_s, url, indent);
        }

        if let Some(name) = balloon.placemark.name {
            self.add_line(&mut out_s, name, indent);
        }

        if let Some(address) = balloon.placemark.address {
            self.add_line(&mut out_s, address, indent);
        }

        if let Some(state) = balloon.placemark.state {
            self.add_line(&mut out_s, state, indent);
        }

        if let Some(city) = balloon.placemark.city {
            self.add_line(&mut out_s, city, indent);
        }

        if let Some(iso_country_code) = balloon.placemark.iso_country_code {
            self.add_line(&mut out_s, iso_country_code, indent);
        }

        if let Some(postal_code) = balloon.placemark.postal_code {
            self.add_line(&mut out_s, postal_code, indent);
        }

        if let Some(country) = balloon.placemark.country {
            self.add_line(&mut out_s, country, indent);
        }

        if let Some(street) = balloon.placemark.street {
            self.add_line(&mut out_s, street, indent);
        }

        if let Some(sub_administrative_area) = balloon.placemark.sub_administrative_area {
            self.add_line(&mut out_s, sub_administrative_area, indent);
        }

        if let Some(sub_locality) = balloon.placemark.sub_locality {
            self.add_line(&mut out_s, sub_locality, indent);
        }

        // We want to keep the newlines between blocks, but the last one should be removed
        out_s.strip_suffix('\n').unwrap_or(&out_s).to_string()
    }

    fn format_handwriting(
        &self,
        msg: &Message,
        balloon: &HandwrittenMessage,
        indent: &str,
    ) -> String {
        match self.config.options.attachment_manager.mode {
            AttachmentManagerMode::Disabled => balloon
                .render_ascii(40)
                .replace('\n', &format!("{indent}\n")),
            _ => self
                .config
                .options
                .attachment_manager
                .handle_handwriting(msg, balloon, self.config)
                .map(|filepath| {
                    self.config
                        .relative_path(PathBuf::from(&filepath))
                        .unwrap_or(filepath.display().to_string())
                })
                .map(|filepath| format!("{indent}{filepath}"))
                .unwrap_or_else(|| {
                    balloon
                        .render_ascii(40)
                        .replace('\n', &format!("{indent}\n"))
                }),
        }
    }

    fn format_digital_touch(&self, _: &Message, balloon: &DigitalTouch, indent: &str) -> String {
        format!("{indent}Digital Touch Message: {balloon:?}")
    }

    fn format_apple_pay(&self, balloon: &AppMessage, indent: &str) -> String {
        let mut out_s = String::from(indent);
        if let Some(caption) = balloon.caption {
            out_s.push_str(caption);
            out_s.push_str(" transaction: ");
        }

        if let Some(ldtext) = balloon.ldtext {
            out_s.push_str(ldtext);
        } else {
            out_s.push_str("unknown amount");
        }

        out_s
    }

    fn format_fitness(&self, balloon: &AppMessage, indent: &str) -> String {
        let mut out_s = String::from(indent);
        if let Some(app_name) = balloon.app_name {
            out_s.push_str(app_name);
            out_s.push_str(" message: ");
        }
        if let Some(ldtext) = balloon.ldtext {
            out_s.push_str(ldtext);
        } else {
            out_s.push_str("unknown workout");
        }
        out_s
    }

    fn format_slideshow(&self, balloon: &AppMessage, indent: &str) -> String {
        let mut out_s = String::from(indent);
        if let Some(ldtext) = balloon.ldtext {
            out_s.push_str("Photo album: ");
            out_s.push_str(ldtext);
        }

        if let Some(url) = balloon.url {
            out_s.push(' ');
            out_s.push_str(url);
        }

        out_s
    }

    fn format_find_my(&self, balloon: &AppMessage, indent: &'a str) -> String {
        let mut out_s = String::from(indent);
        if let Some(app_name) = balloon.app_name {
            out_s.push_str(app_name);
            out_s.push_str(": ");
        }

        if let Some(ldtext) = balloon.ldtext {
            out_s.push(' ');
            out_s.push_str(ldtext);
        }

        out_s
    }

    fn format_check_in(&self, balloon: &AppMessage, indent: &'a str) -> String {
        let mut out_s = String::from(indent);

        out_s.push_str(balloon.caption.unwrap_or("Check In"));

        let metadata: HashMap<&str, &str> = balloon.parse_query_string();

        // Before manual check-in
        if let Some(date_str) = metadata.get("estimatedEndTime") {
            // Parse the estimated end time from the message's query string
            let date_stamp = date_str.parse::<f64>().unwrap_or(0.) as i64 * TIMESTAMP_FACTOR;
            let date_time = get_local_time(&date_stamp, &0);
            let date_string = format(&date_time);

            out_s.push_str("\nExpected at ");
            out_s.push_str(&date_string);
        }
        // Expired check-in
        else if let Some(date_str) = metadata.get("triggerTime") {
            // Parse the estimated end time from the message's query string
            let date_stamp = date_str.parse::<f64>().unwrap_or(0.) as i64 * TIMESTAMP_FACTOR;
            let date_time = get_local_time(&date_stamp, &0);
            let date_string = format(&date_time);

            out_s.push_str("\nWas expected at ");
            out_s.push_str(&date_string);
        }
        // Accepted check-in
        else if let Some(date_str) = metadata.get("sendDate") {
            // Parse the estimated end time from the message's query string
            let date_stamp = date_str.parse::<f64>().unwrap_or(0.) as i64 * TIMESTAMP_FACTOR;
            let date_time = get_local_time(&date_stamp, &0);
            let date_string = format(&date_time);

            out_s.push_str("\nChecked in at ");
            out_s.push_str(&date_string);
        }

        out_s
    }

    fn format_generic_app(
        &self,
        balloon: &AppMessage,
        bundle_id: &str,
        _: &mut Vec<Attachment>,
        indent: &str,
    ) -> String {
        let mut out_s = String::from(indent);

        if let Some(name) = balloon.app_name {
            out_s.push_str(name);
        } else {
            out_s.push_str(bundle_id);
        }

        if !out_s.is_empty() {
            out_s.push_str(" message:\n");
        }

        if let Some(title) = balloon.title {
            self.add_line(&mut out_s, title, indent);
        }

        if let Some(subtitle) = balloon.subtitle {
            self.add_line(&mut out_s, subtitle, indent);
        }

        if let Some(caption) = balloon.caption {
            self.add_line(&mut out_s, caption, indent);
        }

        if let Some(subcaption) = balloon.subcaption {
            self.add_line(&mut out_s, subcaption, indent);
        }

        if let Some(trailing_caption) = balloon.trailing_caption {
            self.add_line(&mut out_s, trailing_caption, indent);
        }

        if let Some(trailing_subcaption) = balloon.trailing_subcaption {
            self.add_line(&mut out_s, trailing_subcaption, indent);
        }

        // We want to keep the newlines between blocks, but the last one should be removed
        out_s.strip_suffix('\n').unwrap_or(&out_s).to_string()
    }
}

impl TXT<'_> {
    fn get_time(&self, message: &Message) -> String {
        let mut date = format(&message.date(&self.config.offset));
        let read_after = message.time_until_read(&self.config.offset);
        if let Some(time) = read_after {
            if !time.is_empty() {
                let who = if message.is_from_me() {
                    "them"
                } else {
                    self.config.options.custom_name.as_deref().unwrap_or("you")
                };
                date.push_str(&format!(" (Read by {who} after {time})"));
            }
        }
        date
    }

    fn add_line(&self, string: &mut String, part: &str, indent: &str) {
        if !part.is_empty() {
            string.push_str(indent);
            string.push_str(part);
            string.push('\n');
        }
    }
}
