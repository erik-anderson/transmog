# Saving and inspecting traces

Use **Traffic → Save trace…** in the main window or a Capture viewer. It saves
all retained entries across pages and search results; removed entries are excluded.
Undo a removal before saving if those entries should be included. The file dialog
confirms overwriting, and an existing destination is replaced only after a complete
successful write. Failed reads or writes leave that destination intact.

Compression is selected by default for sharing. Use .tmcap.gz for compressed
native files or .tmcap for native files. Bodies stream from retained storage;
large captures are not collected into one in-memory buffer. Active, missing or
incomplete bodies remain explicit and are counted in the save report. Review
private headers and bodies before sharing.

**Include this computer’s network configuration** is optional and unchecked. It
records bounded ipconfig /all output on Windows, interface/DNS/route context on
macOS, or ip and resolvectl output on Linux. Collection time, platform and failures
stay with this context. This option also exists when recording from Capture files,
and the CLI offers --include-network-context.

Saving merged native traces preserves each original source's metadata, original
request identifier, raw imported heads, timers and terminal time. Select a request
and choose **Trace metadata** to reach its associated source. The source selector
also offers the saved workspace's own context; network configuration from the
saving computer does not replace the original machine's configuration. Imported
source names, paths and session counts describe the original capture.

Optional capture-level context also survives conventional and extended SAZ
conversions in a namespaced JSON member ignored by ordinary SAZ readers. Native
files preserve the full original source associations and measured transport
observations; SAZ conversion reports its compatibility limitations.

Imports bound the index for each source and the combined index and metadata
across merged files. Publication is atomic: rejected files add neither partial
traffic nor partial source metadata. Removing an entry keeps evidence available
for Undo, so its imported index remains retained until its viewer closes.
