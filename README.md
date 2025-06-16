# imessage-undeleter
Tracks your iMessage database and logs messages which have been deleted and what they are

## Credits
This project heavily relied on ReagentX's [imessage-exporter](https://github.com/ReagentX/imessage-exporter). I made some modifications to the imessage-database library to support limiting message results, and I used imessage-exporter to guide my imessage-undeleter code.

Much of the old code in imessage-exporter still exists, I just don't really care to clean things up right now. This isn't prod. 

## Usage
```bash
run -- -t "phone_number" -n "how many messages back to check"
```

This will create a directory in your working directory called undeleted_messages.

## Improvements Over Previous Python Version
Since this uses the imessage-database library, the reverse engineering is far less scuffed, and we can access more features than just text. Most notibly, we keep track of attachments which were deleted.