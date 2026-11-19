package llc.improv.app.workbook

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class ModelStoreTest {

    @get:Rule
    val tempFolder = TemporaryFolder()

    private fun store() = ModelStore(tempFolder.root)

    @Test
    fun createsBackingDirectoryWhenMissing() {
        val nested = File(tempFolder.root, "a/b/c")
        assertFalse(nested.exists())
        ModelStore(nested)
        assertTrue(nested.isDirectory)
    }

    @Test
    fun rejectsNonDirectoryBackingPath() {
        val file = tempFolder.newFile("not-a-dir")
        assertThrows(IllegalArgumentException::class.java) { ModelStore(file) }
    }

    @Test
    fun listIsEmptyForFreshStore() {
        assertTrue(store().list().isEmpty())
    }

    @Test
    fun createSlugifiesNameAndWritesSidecarWithoutDbFile() {
        val s = store()
        val id = s.create("Q4 Revenue Plan!")
        assertEquals("q4-revenue-plan", id)
        // create() does not make the .db; only the sidecar exists so the entry lists.
        assertFalse(s.exists(id))
        assertTrue(File(tempFolder.root, "$id.meta").isFile)
        val entry = s.list().single()
        assertEquals("Q4 Revenue Plan!", entry.displayName)
        assertEquals(id, entry.id)
    }

    @Test
    fun createFallsBackWhenNameHasNoUsableCharacters() {
        val id = store().create("!!!")
        assertEquals("workbook", id)
    }

    @Test
    fun createDeduplicatesCollidingSlugs() {
        val s = store()
        val first = s.create("Budget")
        val second = s.create("Budget")
        val third = s.create("Budget")
        assertEquals("budget", first)
        assertEquals("budget-2", second)
        assertEquals("budget-3", third)
        assertEquals(3, s.list().size)
    }

    @Test
    fun dbPathForBuildsExpectedPath() {
        val s = store()
        assertEquals(File(tempFolder.root, "sales.db").absolutePath, s.dbPathFor("sales"))
    }

    @Test
    fun existsTracksRealDbFile() {
        val s = store()
        assertFalse(s.exists("sales"))
        File(s.dbPathFor("sales")).writeBytes(byteArrayOf(1, 2, 3))
        assertTrue(s.exists("sales"))
    }

    @Test
    fun renameChangesDisplayNameButKeepsIdAndSummary() {
        val s = store()
        val id = s.create("Draft", summary = "2 categories × 1 measure")
        s.rename(id, "Final Plan")
        val entry = s.list().single()
        assertEquals(id, entry.id)
        assertEquals("Final Plan", entry.displayName)
        assertEquals("2 categories × 1 measure", entry.summary)
    }

    @Test
    fun renameRejectsBlankName() {
        val s = store()
        val id = s.create("Draft")
        assertThrows(IllegalArgumentException::class.java) { s.rename(id, "   ") }
    }

    @Test
    fun updateSummaryIsReflectedInListing() {
        val s = store()
        val id = s.create("Plan")
        s.updateSummary(id, "Plan", "4 categories × 9 measures")
        assertEquals("4 categories × 9 measures", s.list().single().summary)
    }

    @Test
    fun deleteRemovesDbAndSidecarAndIsIdempotent() {
        val s = store()
        val id = s.create("Plan")
        File(s.dbPathFor(id)).writeBytes(byteArrayOf(0))
        assertTrue(s.delete(id))
        assertFalse(File(s.dbPathFor(id)).exists())
        assertFalse(File(tempFolder.root, "$id.meta").exists())
        // Second delete of a now-unknown id is a harmless no-op.
        assertFalse(s.delete(id))
    }

    @Test
    fun listSurfacesDbWithoutSidecarUsingIdFallback() {
        val s = store()
        File(tempFolder.root, "orphan.db").writeBytes(byteArrayOf(9))
        val entry = s.list().single()
        assertEquals("orphan", entry.id)
        assertEquals("orphan", entry.displayName)
        assertEquals("Not yet opened", entry.summary)
    }

    @Test
    fun listOrdersNewestFirst() {
        val s = store()
        val older = File(s.dbPathFor("older")).apply { writeBytes(byteArrayOf(1)) }
        val newer = File(s.dbPathFor("newer")).apply { writeBytes(byteArrayOf(2)) }
        older.setLastModified(1_000L)
        newer.setLastModified(2_000L)
        val ids = s.list().map { it.id }
        assertEquals(listOf("newer", "older"), ids)
    }

    @Test
    fun ignoresNonWorkbookFilesButSurfacesSidecarOnlyWorkbooks() {
        val s = store()
        File(tempFolder.root, "notes.txt").writeText("hi")
        File(tempFolder.root, "stray.meta").writeText("Stray\nsummary")
        // notes.txt is not a workbook; the lone sidecar is a half-created one.
        val entries = s.list()
        assertEquals(1, entries.size)
        assertEquals("stray", entries.single().id)
        assertEquals("Stray", entries.single().displayName)
    }

    @Test
    fun sidecarRoundTripsMultibyteDisplayNames() {
        val s = store()
        val id = s.create("売上 × Margin")
        // The slug drops non-ASCII, but the display name must round-trip verbatim.
        assertNotEquals("", id)
        assertEquals("売上 × Margin", s.list().single().displayName)
    }
}
