# Copying inspection values

Right-click a read-only inspection field or tree item to copy it
without first selecting its text. The menu targets the item under the pointer;
copying does not change the selected request, tree field or stream message.
Shift+F10 or the Context Menu key opens the same actions for a focused item.
Escape dismisses the menu and returns focus.
Pointer menus open at the cursor and move only as far as their rendered size
requires to fit inside the window. Keyboard menus open beside the focused item. Inspection, selected-text and Traffic
menus measure their actual content, including changes after opening and zoom.
Arrow keys, Home and End move within menus. Escape restores the source focus;
clicking elsewhere keeps the clicked control focused. A brief **Copied.** message
confirms successful inspection and selected-text copying without moving focus.

| Action | Clipboard text |
| --- | --- |
| Copy name | The complete field label. |
| Copy value | The complete serialized scalar or byte value. |
| Copy name and value | `Name: value`. |
| Copy subtree | The node and its descendants, with two spaces per layer. |

For example:

```text
Message
  Subject: July report
  Flags: Read (1)
  Payload: 00 0A FF
```

Text values remain readable. Scalar serialization escapes backslashes, embedded
newlines, tabs, control characters and unpaired UTF-16 surrogates. Valid Unicode
characters remain readable. Wide protocol integers retain their exact
decimal strings; enum and flag values retain their names and raw values. Byte
arrays become uppercase hexadecimal pairs separated by spaces. Whole text/report
actions preserve their text block's line breaks. Clipboard content is plain text.

Actions follow their context. Objects, arrays and Auth finding headings without
a scalar value omit Copy value and Copy name and value. Leaves omit Copy subtree.
Unnamed text blocks offer Copy text; named previews and converter output identify
what is copied. A text block's display label is not a field name: Body previews,
Raw data and converter output offer only their text-copy action.
Key-value groups with multiple fields offer Copy section. Tables offer field or
row actions where applicable. Redacted/unavailable values explain why value
actions are disabled, while their names remain copyable.

## Extension trees and tables

The shared host renderer supplies these actions for every extension, including
the bundled MAPI/HTTP visualizer. Copying uses typed values rather than shortened
tree/table cells. Byte values read only their declared source ranges, including
validated derived buffers, rather than a containing HTTP message by default.

Copy subtree includes collapsed and deferred descendants. Explicit copying may
load deferred children while retaining the visible expansion set, selection,
view mode and scroll position. It does not rerun discovery across other traffic.
Duplicate names and the extension's ordering are preserved.
When a deep binary view flattens descendants into separate rows, the enclosing
container row is explicitly labeled as wire bytes. Copy value reads its complete
encoded range; it does not offer an empty decoded subtree.

## Auth and headers

Individual Auth value actions copy the actual retained value, including masked
sensitive material, without revealing it onscreen. Copy subtree always masks
sensitive fields, even if a field was revealed. Copy subtree including secrets is
a separate explicit action, shown only when readable sensitive fields exist.
Neither action reconstructs redacted data. See [Auth inspection](auth-inspection.md).

Header actions read the complete original occurrence in the selected message
stage. They retain original name casing and keep duplicate occurrences distinct,
independently of paging, display truncation or size sorting. Non-Unicode header
values use hexadecimal text. Open in Format converter remains available for
eligible values. The existing bulk Copy headers action retains its HTTP format.

## Other inspection surfaces

The same conventions cover request details, timing and
source metadata; connection/TLS/certificate fields; comparison values; retained
WebSocket/SSE messages; body previews and Raw data; diagnostic/report text;
capture/package/extension metadata; and read-only converter output. The Traffic
list keeps its existing request context menu, Copy submenu and row-targeting
selection behavior. Editors retain
their ordinary editing shortcuts and menus; existing text-selection copying is
preserved. Hex keeps its specialized byte-range/encoding and offset actions.

Copy body preview copies the entire loaded source representation, including
folded and offscreen text, which can be shorter than the retained body. It uses
the underlying text rather than syntax spans, line numbers or editor messages.
Formatted JSON also offers **Copy original text preview** for that same decoded
preview boundary before formatting. Right-clicking inside a source selection,
or opening its keyboard menu with a selection, puts **Copy** first and offers
**Open in Format converter** for that selection. Right-click preserves the text
selection and inspected request. Shift+F10 or the Context Menu key opens at the
caret; Escape returns focus to the source editor. Ctrl+C with an empty source
selection leaves the clipboard unchanged. Find's input retains ordinary text
copying. The editable response and script editors retain their own menus.
Previews above the 1 MiB syntax limit use the same copy actions on a selectable
plain-text surface. Pointer menus offer semantic field actions only when their
source location can be mapped reliably; Fields retains those actions directly.

Warnings and missing-body prose offer **Copy explanation**, independently of the
body evidence. Metadata offers **Copy body metadata**. Selected text offers Copy
first; read-only selections inside modal inspectors use the modal's copy menu.
Retained stream-prefix coverage stays explicit. Use the existing
body/message save actions when the complete retained binary representation is
needed. Display masking and clipboard actions do not sanitize retained traffic
or change capture/export redaction.

## JSON body fields

**Fields** and reliably mapped source tokens use the original decoded JSON
preview. Duplicate keys keep their order and remain separate occurrences;
numbers retain their captured spelling, including wide integers, exponents and
negative zero. Copy name, Copy value and Copy name and value follow the scalar
escaping rules above. Containers offer **Copy subtree** using the readable
two-space indentation format, including collapsed descendants.
Display labels identify empty or whitespace-only names and strings; copy actions
still use their original names and values.

**Copy as JSON** copies a valid JSON literal, object or array from its original
source range, retaining token spelling and duplicates. It excludes an enclosing
anti-XSSI prefix. **Copy JSON Pointer** escapes `/` and `~` in property names and
uses array indices. The action is omitted for duplicate-key paths and their
descendants, and for keys that cannot survive plain-text clipboard Unicode or
line-ending handling without changing their identity. A pointer to the root is
the empty string. These actions operate on the bounded preview; Save as remains
the route to a complete retained response.

## Limits and changing evidence

Clipboard text is limited to 16 MiB of UTF-8. Subtrees have at most 10,000 nodes,
depth 32, 256 deferred expansions and a 30-second cooperative copy deadline;
individual extension operations also retain their native parser limits. Source
reads use bounded pages. Header values retain their 8 MiB input limit and a
64 KiB name limit; hexadecimal expansion also counts against clipboard output.
Auth grouped reads retain the analysis's 2,048-field and 8 MiB aggregate limits.

Copy preparation checks its owning pane, evidence and active capability. Closing
the menu, changing evidence or hiding the view prevents late preparation from
writing to the clipboard. Size exhaustion, missing ranges and clipboard errors
produce explicit feedback. No partial/truncated substitute is silently copied.
Unknown backend errors are replaced with static messages so credentials cannot
leak through a copy failure.
