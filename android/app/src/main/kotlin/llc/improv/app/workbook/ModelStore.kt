package llc.improv.app.workbook

import java.io.File

/**
 * A single workbook on disk.  [id] is the stable file-stem used to build the
 * `<id>.db` path; [displayName] is the human label shown in the library; and
 * [summary] is a short one-line description (e.g. `"3 categories × 6 measures"`)
 * cached in a sidecar so the library can list without opening every engine.
 */
data class WorkbookEntry(
    val id: String,
    val displayName: String,
    val summary: String,
    val lastModified: Long,
)

/**
 * File-level workbook management over a directory of `<id>.db` Mentat stores,
 * each paired with an `<id>.meta` text sidecar (line 1 = display name, line 2 =
 * summary).  This type is deliberately Android-free so it can be unit-tested
 * against a plain temp directory; the engine lifecycle (open/create/seed) is
 * the ViewModel's job, not this store's.
 *
 * NB: the caller supplies the backing directory, which must already exist and
 * be writable.  On Android that is `filesDir`; in tests it is a temp dir.
 */
class ModelStore(private val dir: File) {

    init {
        if (!dir.exists()) {
            dir.mkdirs()
        }
        require(dir.isDirectory) { "ModelStore directory is not a directory: $dir" }
    }

    /** Absolute path of the `.db` store for [id], whether or not it exists yet. */
    fun dbPathFor(id: String): String = File(dir, "$id.db").absolutePath

    /** True when a `.db` store already exists for [id]. */
    fun exists(id: String): Boolean = File(dir, "$id.db").isFile

    /**
     * Lists every workbook in the directory, newest first.  A workbook is any id
     * that has a `.db` store, a `.meta` sidecar, or both: a freshly-created
     * workbook (sidecar written, engine not yet opened) still lists, and so does
     * an externally-dropped `.db` with no sidecar (falling back to its id for the
     * display name and a neutral summary).
     */
    fun list(): List<WorkbookEntry> {
        val files = dir.listFiles { f ->
            f.isFile && (f.name.endsWith(".db") || f.name.endsWith(".meta"))
        } ?: emptyArray()
        return files
            .map { it.nameWithoutExtension }
            .distinct()
            .map { id ->
                val meta = readMeta(id)
                val db = File(dir, "$id.db")
                // A sidecar-only workbook sorts by its sidecar's mtime instead.
                val mtime = if (db.isFile) db.lastModified() else File(dir, "$id.meta").lastModified()
                WorkbookEntry(
                    id = id,
                    displayName = meta?.first ?: id,
                    summary = meta?.second ?: "Not yet opened",
                    lastModified = mtime,
                )
            }
            .sortedByDescending { it.lastModified }
    }

    /**
     * Allocates a new workbook id for [displayName] and writes its sidecar, but
     * does NOT create the `.db` store (the ViewModel does that by opening the
     * engine).  The id is a filesystem-safe slug of the name, made unique with a
     * numeric suffix when a store of that stem already exists.  Returns the id.
     */
    fun create(displayName: String, summary: String = "New workbook"): String {
        val base = slugify(displayName).ifEmpty { "workbook" }
        var candidate = base
        var n = 2
        while (File(dir, "$candidate.db").exists() || File(dir, "$candidate.meta").exists()) {
            candidate = "$base-$n"
            n += 1
        }
        writeMeta(candidate, displayName.trim().ifEmpty { candidate }, summary)
        return candidate
    }

    /** Renames the workbook's display name in its sidecar.  The id (and db file) stay put. */
    fun rename(id: String, newDisplayName: String) {
        val trimmed = newDisplayName.trim()
        require(trimmed.isNotEmpty()) { "Workbook name must not be empty" }
        val existing = readMeta(id)
        writeMeta(id, trimmed, existing?.second ?: "")
    }

    /** Updates the cached one-line summary for [id] (called after the engine loads). */
    fun updateSummary(id: String, displayName: String, summary: String) {
        writeMeta(id, displayName, summary)
    }

    /**
     * Deletes the `.db` store and its sidecar.  Returns true when at least one
     * file existed and was removed; a no-op delete of an unknown id returns false
     * rather than throwing, so the caller can treat it as idempotent.
     */
    fun delete(id: String): Boolean {
        val db = File(dir, "$id.db")
        val meta = File(dir, "$id.meta")
        val hadDb = db.exists() && db.delete()
        val hadMeta = meta.exists() && meta.delete()
        return hadDb || hadMeta
    }

    /** Reads the sidecar for [id] as (displayName, summary), or null when absent. */
    private fun readMeta(id: String): Pair<String, String>? {
        val meta = File(dir, "$id.meta")
        if (!meta.isFile) return null
        val lines = meta.readText().split('\n')
        val name = lines.getOrNull(0)?.trim().orEmpty().ifEmpty { id }
        val summary = lines.getOrNull(1)?.trim().orEmpty()
        return name to summary
    }

    /** Writes the sidecar for [id] as two text lines. */
    private fun writeMeta(id: String, displayName: String, summary: String) {
        File(dir, "$id.meta").writeText("${displayName.trim()}\n${summary.trim()}")
    }

    private companion object {
        /** Lowercase, keep alphanumerics, collapse everything else to single dashes. */
        fun slugify(name: String): String =
            name.trim()
                .lowercase()
                .map { if (it.isLetterOrDigit()) it else '-' }
                .joinToString("")
                .trim('-')
                .replace(Regex("-+"), "-")
    }
}
