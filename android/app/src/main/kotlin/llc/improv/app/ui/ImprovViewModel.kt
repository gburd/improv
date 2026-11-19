package llc.improv.app.ui

import androidx.lifecycle.ViewModel
import llc.improv.app.DirectViewportReader
import llc.improv.app.populateDirectByteBuffer
import llc.improv.app.workbook.ModelStore
import llc.improv.app.workbook.WorkbookEntry
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import uniffi.improv_android_ffi.AxisOrientation
import uniffi.improv_android_ffi.CategoryTile
import uniffi.improv_android_ffi.FormulaEntry
import uniffi.improv_android_ffi.ImprovMobileEngine
import uniffi.improv_android_ffi.MeasureSummary
import uniffi.improv_android_ffi.SliceAnalyticsSummary
import uniffi.improv_android_ffi.ViewportCell
import uniffi.improv_android_ffi.ViewportSnapshot
import java.nio.ByteBuffer

/**
 * The spreadsheet UI state for the currently open workbook.  Every field
 * defaults to a neutral empty value; real content is seeded from the opened
 * model in [ImprovViewModel.refreshFromEngine].  Nothing here is tied to a
 * particular demo model.
 */
data class ImprovUiState(
    val activeModelId: String? = null,
    val activeViewName: String = "",
    val rowCategories: List<CategoryTile> = emptyList(),
    val columnCategories: List<CategoryTile> = emptyList(),
    val pageCategories: List<CategoryTile> = emptyList(),
    val measures: List<MeasureSummary> = emptyList(),
    val formulas: List<FormulaEntry> = emptyList(),
    val viewport: ViewportSnapshot = ViewportSnapshot(
        rowHeaders = emptyList(),
        colHeaders = emptyList(),
        totalRows = 0u,
        totalCols = 0u,
        activeMeasureId = "",
        activeMeasureName = "",
        cells = emptyList(),
    ),
    val selectedRow: Int = 0,
    val selectedCol: Int = 0,
    val selectedCellCoordinate: String = "",
    val selectedCellFormatted: String = "",
    val selectedCellEditBuffer: String = "",
    val selectedCellIsDerived: Boolean = false,
    val scrollRowIndex: Int = 0,
    val scrollColIndex: Int = 0,
    val formulaInput: String = "",
    val autocompleteSuggestions: List<String> = emptyList(),
    val formulaError: String? = null,
    val directBufferBytesWritten: Int = 0,
    val directBufferSelectedValue: Double = Double.NaN,
    val activeAnalyticsMeasure: String = "",
    val sliceAnalytics: SliceAnalyticsSummary = SliceAnalyticsSummary(
        measureName = "",
        cellCount = 0u,
        sum = 0.0,
        mean = 0.0,
        median = 0.0,
        min = 0.0,
        max = 0.0,
        stdDev = 0.0,
        seriesLabels = emptyList(),
        seriesValues = emptyList(),
    ),
    val csvImportExportBuffer: String = "",
    val statusMessage: String = "",
)

/** The workbook library state: what the home screen lists when no model is open. */
data class LibraryUiState(
    val workbooks: List<WorkbookEntry> = emptyList(),
    val errorMessage: String? = null,
)

/**
 * Owns the workbook library and the single live engine for the open model.
 *
 * The ViewModel no longer takes one engine for life.  It holds a [ModelStore]
 * and an engine factory, opens an engine when the user opens a workbook, and
 * closes it when they return to the library, so one process can move between
 * many `.db` stores.  Both the store and the factory are injected, which keeps
 * the model-switching and seeding logic unit-testable: a test passes a
 * temp-dir [ModelStore] and the real [ImprovMobileEngine.openOrCreate] factory.
 */
class ImprovViewModel(
    private val modelStore: ModelStore,
    private val engineFactory: (String) -> ImprovMobileEngine = { ImprovMobileEngine.openOrCreate(it) },
) : ViewModel() {

    companion object {
        const val KEY_ACTIVE_MODEL_ID = "active_model_id"
    }

    private val directViewportBuffer: ByteBuffer = ByteBuffer.allocateDirect(64 * 1024)
    private val directReader = DirectViewportReader(directViewportBuffer)

    // Null until the user opens or creates a workbook.  Every engine-touching
    // method guards on this so a no-model library state cannot call the FFI.
    private var engine: ImprovMobileEngine? = null

    private val _uiState = MutableStateFlow(ImprovUiState())
    val uiState: StateFlow<ImprovUiState> = _uiState.asStateFlow()

    private val _libraryState = MutableStateFlow(LibraryUiState())
    val libraryState: StateFlow<LibraryUiState> = _libraryState.asStateFlow()

    init {
        refreshLibrary()
    }

    // ---- Workbook library / lifecycle --------------------------------------

    /** Re-reads the workbook directory into [libraryState]. */
    fun refreshLibrary() {
        _libraryState.update {
            try {
                it.copy(workbooks = modelStore.list(), errorMessage = null)
            } catch (e: Exception) {
                it.copy(errorMessage = e.message ?: "Could not list workbooks")
            }
        }
    }

    /** True when a workbook is open and the spreadsheet surface should show. */
    fun hasOpenModel(): Boolean = _uiState.value.activeModelId != null

    /**
     * Opens the workbook [id], switching the live engine to its `.db` store and
     * seeding the UI state from the model.  Safe to call for the already-open id
     * (it simply refreshes).
     */
    fun openModel(id: String) {
        val normalized = id.trim()
        if (normalized.isEmpty()) return
        try {
            closeEngine()
            engine = engineFactory(modelStore.dbPathFor(normalized))
            _uiState.value = ImprovUiState(activeModelId = normalized)
            val name = _libraryState.value.workbooks.firstOrNull { it.id == normalized }?.displayName
                ?: normalized
            refreshFromEngine(selectedRow = 0, selectedCol = 0, status = "Opened '$name'")
            syncSummaryToStore(normalized, name)
        } catch (e: Exception) {
            engine = null
            _uiState.update { ImprovUiState() }
            _libraryState.update { it.copy(errorMessage = "Could not open '$normalized': ${e.message}") }
            refreshLibrary()
        }
    }

    /**
     * Creates a blank-starter workbook named [displayName] and opens it.  The
     * engine's open_or_create auto-seeds the sample model into a new `.db`, so
     * createBlankModel() is called immediately after to overwrite it with the
     * Period x Department starter.
     */
    fun createBlankWorkbook(displayName: String) {
        val name = displayName.trim().ifEmpty { "Untitled Workbook" }
        try {
            val id = modelStore.create(name)
            closeEngine()
            engine = engineFactory(modelStore.dbPathFor(id))
            engine!!.createBlankModel()
            _uiState.value = ImprovUiState(activeModelId = id)
            refreshFromEngine(selectedRow = 0, selectedCol = 0, status = "Created '$name'")
            syncSummaryToStore(id, name)
            refreshLibrary()
        } catch (e: Exception) {
            engine = null
            _libraryState.update { it.copy(errorMessage = "Could not create workbook: ${e.message}") }
        }
    }

    /**
     * Creates an example workbook named [displayName] and opens it.  The
     * auto-seed on a fresh `.db` already IS the sample, so the engine opens
     * directly onto it.
     */
    fun createSampleWorkbook(displayName: String) {
        val name = displayName.trim().ifEmpty { "Sample Workbook" }
        try {
            val id = modelStore.create(name, summary = "Example model")
            closeEngine()
            engine = engineFactory(modelStore.dbPathFor(id))
            // Force the canonical sample even if the db existed from a prior id reuse.
            engine!!.loadSampleModel()
            _uiState.value = ImprovUiState(activeModelId = id)
            refreshFromEngine(selectedRow = 0, selectedCol = 0, status = "Created example '$name'")
            syncSummaryToStore(id, name)
            refreshLibrary()
        } catch (e: Exception) {
            engine = null
            _libraryState.update { it.copy(errorMessage = "Could not create example: ${e.message}") }
        }
    }

    /** Renames the given workbook's display name and refreshes the library. */
    fun renameWorkbook(id: String, newName: String) {
        try {
            modelStore.rename(id, newName)
            if (_uiState.value.activeModelId == id) {
                _uiState.update { it.copy(activeViewName = newName.trim()) }
            }
            refreshLibrary()
        } catch (e: Exception) {
            _libraryState.update { it.copy(errorMessage = e.message ?: "Could not rename workbook") }
        }
    }

    /** Deletes the given workbook; closes it first when it is the open one. */
    fun deleteWorkbook(id: String) {
        if (_uiState.value.activeModelId == id) {
            closeModel()
        }
        modelStore.delete(id)
        refreshLibrary()
    }

    /** Closes the open workbook and returns to the library. */
    fun closeModel() {
        closeEngine()
        _uiState.value = ImprovUiState()
        refreshLibrary()
    }

    private fun closeEngine() {
        engine?.close()
        engine = null
    }

    /** Writes the opened model's category x measure shape back to the sidecar. */
    private fun syncSummaryToStore(id: String, displayName: String) {
        val e = engine ?: return
        try {
            val cats = e.getCategories().size
            val meas = e.getMeasures().size
            modelStore.updateSummary(id, displayName, "$cats categories × $meas measures")
        } catch (_: Exception) {
            // A summary refresh is best-effort; a failure must not block opening.
        }
    }

    /** Re-syncs the open model's summary, resolving its name from the sidecar. */
    private fun syncOpenModelSummary() {
        val id = _uiState.value.activeModelId ?: return
        val name = modelStore.list().firstOrNull { it.id == id }?.displayName ?: id
        syncSummaryToStore(id, name)
    }

    // ---- Model editing (guarded on an open engine) -------------------------

    fun refreshFromEngine(
        selectedRow: Int = _uiState.value.selectedRow,
        selectedCol: Int = _uiState.value.selectedCol,
        status: String? = null,
    ) {
        val engine = engine ?: return
        val allCategories = engine.getCategories()
        val rowCats = allCategories
            .filter { it.orientation == AxisOrientation.ROW }
            .sortedBy { it.positionIndex }
        val colCats = allCategories
            .filter { it.orientation == AxisOrientation.COLUMN }
            .sortedBy { it.positionIndex }
        val pageCats = allCategories
            .filter { it.orientation == AxisOrientation.PAGE }
            .sortedBy { it.positionIndex }

        val measures = engine.getMeasures()
        val formulas = engine.getFormulas()
        val vp = engine.getViewport(0u, 100u, 0u, 50u)

        val bytesWritten = try {
            engine.populateDirectByteBuffer(
                buffer = directViewportBuffer,
                startRow = 0u,
                rowCount = 100u,
                startCol = 0u,
                colCount = 50u,
            )
        } catch (_: Throwable) {
            0
        }

        val totalRows = vp.rowHeaders.size
        val totalCols = vp.colHeaders.size
        val clampedRow = if (totalRows > 0) selectedRow.coerceIn(0, totalRows - 1) else 0
        val clampedCol = if (totalCols > 0) selectedCol.coerceIn(0, totalCols - 1) else 0

        val selectedCell = vp.cellAt(clampedRow, clampedCol)
        val semanticCoord = buildSemanticCoordinate(
            rowCats = rowCats,
            colCats = colCats,
            pageCats = pageCats,
            viewport = vp,
            row = clampedRow,
            col = clampedCol,
            cell = selectedCell,
        )

        val directVal = if (bytesWritten >= 16 && clampedRow < directReader.rows && clampedCol < directReader.cols) {
            directReader.numericValue(clampedRow, clampedCol)
        } else {
            selectedCell?.numericValue ?: Double.NaN
        }

        val current = _uiState.value
        val suggestions = engine.autocompleteFormula(current.formulaInput)
        // Analytics defaults to the selected cell's measure, else the previously
        // chosen measure if it still exists, else the model's last measure.
        val targetAnalyticsMeasure = selectedCell?.measureName?.takeIf { it.isNotBlank() }
            ?: current.activeAnalyticsMeasure.takeIf { name ->
                name.isNotBlank() && measures.any { it.name.equals(name, ignoreCase = true) }
            }
            ?: measures.lastOrNull()?.name
        val analyticsSummary = targetAnalyticsMeasure?.let { engine.getSliceAnalytics(it) }

        // The view name and formula hint derive from the model's real shape, not
        // a demo literal, and only fill in while they are still empty.
        val derivedViewName = current.activeViewName.ifBlank { deriveViewName(rowCats, colCats) }
        val derivedFormulaInput = current.formulaInput.ifBlank { deriveFormulaHint(measures) }

        _uiState.update {
            it.copy(
                activeViewName = derivedViewName,
                rowCategories = rowCats,
                columnCategories = colCats,
                pageCategories = pageCats,
                measures = measures,
                formulas = formulas,
                viewport = vp,
                selectedRow = clampedRow,
                selectedCol = clampedCol,
                selectedCellCoordinate = semanticCoord,
                selectedCellFormatted = selectedCell?.formatted.orEmpty(),
                selectedCellEditBuffer = selectedCell?.formatted?.replace(",", "").orEmpty(),
                selectedCellIsDerived = selectedCell?.isDerived ?: false,
                formulaInput = derivedFormulaInput,
                autocompleteSuggestions = suggestions,
                directBufferBytesWritten = bytesWritten,
                directBufferSelectedValue = directVal,
                activeAnalyticsMeasure = analyticsSummary?.measureName.orEmpty(),
                sliceAnalytics = analyticsSummary ?: it.sliceAnalytics,
                statusMessage = status ?: it.statusMessage,
            )
        }
    }

    /** "Time × Product" style name from the first row/column categories, or a neutral label. */
    private fun deriveViewName(rowCats: List<CategoryTile>, colCats: List<CategoryTile>): String {
        val rowName = rowCats.firstOrNull()?.name
        val colName = colCats.firstOrNull()?.name
        return when {
            rowName != null && colName != null -> "$rowName × $colName"
            rowName != null -> rowName
            colName != null -> colName
            else -> "Workbook"
        }
    }

    /** A starter formula hint from the model's last measure, or empty for a bare model. */
    private fun deriveFormulaHint(measures: List<MeasureSummary>): String {
        val name = measures.lastOrNull()?.name ?: return ""
        return "$name = "
    }

    private fun buildSemanticCoordinate(
        rowCats: List<CategoryTile>,
        colCats: List<CategoryTile>,
        pageCats: List<CategoryTile>,
        viewport: ViewportSnapshot,
        row: Int,
        col: Int,
        cell: ViewportCell?,
    ): String {
        val parts = mutableListOf<String>()
        val rowHeader = viewport.rowHeaders.getOrNull(row).orEmpty()
        val colHeader = viewport.colHeaders.getOrNull(col).orEmpty()

        val rowCategoryLabel = rowHeader.substringBefore(" · ").trim()
        val rowCatValues = rowCategoryLabel.split(" / ")
        rowCats.forEachIndexed { idx, tile ->
            val v = rowCatValues.getOrNull(idx)?.takeIf { it.isNotBlank() }
                ?: tile.items.firstOrNull()?.name
                ?: "*"
            parts.add("${tile.name}:$v")
        }

        val colCatValues = colHeader.split(" / ")
        colCats.forEachIndexed { idx, tile ->
            val v = colCatValues.getOrNull(idx)?.takeIf { it.isNotBlank() }
                ?: tile.items.firstOrNull()?.name
                ?: "*"
            parts.add("${tile.name}:$v")
        }

        pageCats.forEach { tile ->
            val selectedName = tile.items
                .firstOrNull { it.id == tile.selectedPageItemId }
                ?.name
                ?: tile.items.firstOrNull()?.name
                ?: "*"
            parts.add("${tile.name}:$selectedName")
        }

        val measureName = cell?.measureName?.takeIf { it.isNotBlank() }
            ?: rowHeader.substringAfter(" · ", missingDelimiterValue = "Value")
        parts.add("Measure:$measureName")

        return parts.joinToString("|")
    }

    fun selectCell(row: Int, col: Int) {
        refreshFromEngine(selectedRow = row, selectedCol = col)
    }

    fun moveCellFocus(deltaRow: Int, deltaCol: Int) {
        val state = _uiState.value
        val maxRow = (state.viewport.rowHeaders.size - 1).coerceAtLeast(0)
        val maxCol = (state.viewport.colHeaders.size - 1).coerceAtLeast(0)
        val nextRow = (state.selectedRow + deltaRow).coerceIn(0, maxRow)
        val nextCol = (state.selectedCol + deltaCol).coerceIn(0, maxCol)
        selectCell(nextRow, nextCol)
    }

    fun moveTabFocus(reverse: Boolean = false) {
        val state = _uiState.value
        val rows = state.viewport.rowHeaders.size.coerceAtLeast(1)
        val cols = state.viewport.colHeaders.size.coerceAtLeast(1)
        val flat = state.selectedRow * cols + state.selectedCol
        val nextFlat = if (reverse) {
            (flat - 1 + rows * cols) % (rows * cols)
        } else {
            (flat + 1) % (rows * cols)
        }
        selectCell(nextFlat / cols, nextFlat % cols)
    }

    fun updateScrollIndices(rowIndex: Int, colIndex: Int) {
        _uiState.update {
            it.copy(scrollRowIndex = rowIndex, scrollColIndex = colIndex)
        }
    }

    fun pivotCategory(
        categoryId: String,
        targetAxis: AxisOrientation,
        newIndex: Int = 0,
    ) {
        val engine = engine ?: return
        engine.pivotCategory(categoryId, targetAxis, newIndex.coerceAtLeast(0).toUInt())
        val catName = (_uiState.value.rowCategories + _uiState.value.columnCategories + _uiState.value.pageCategories)
            .firstOrNull { it.id == categoryId || it.name.equals(categoryId, ignoreCase = true) }
            ?.name ?: categoryId
        refreshFromEngine(status = "Pivoted '$catName' to ${targetAxis.name} axis")
    }

    fun cycleCategoryAxis(tile: CategoryTile) {
        val nextAxis = when (tile.orientation) {
            AxisOrientation.ROW -> AxisOrientation.COLUMN
            AxisOrientation.COLUMN -> AxisOrientation.PAGE
            AxisOrientation.PAGE -> AxisOrientation.ROW
        }
        pivotCategory(tile.id, nextAxis, 0)
    }

    fun selectPageItem(categoryId: String, itemId: String) {
        val engine = engine ?: return
        engine.selectPageItem(categoryId, itemId)
        refreshFromEngine(status = "Updated page slice filter")
    }

    fun addCategory(name: String, itemsCsv: String, orientation: AxisOrientation) {
        val engine = engine ?: return
        val trimmed = name.trim()
        if (trimmed.isEmpty()) {
            _uiState.update { it.copy(formulaError = "Category name is required") }
            return
        }
        val items = itemsCsv
            .split(",")
            .map { it.trim() }
            .filter { it.isNotEmpty() }
        try {
            val tile = engine.addCategory(trimmed, items, orientation)
            _uiState.update { it.copy(formulaError = null) }
            refreshFromEngine(status = "Added category '${tile.name}' (${tile.items.size} items) on ${orientation.name}")
            syncOpenModelSummary()
        } catch (e: Exception) {
            _uiState.update { it.copy(formulaError = e.message ?: "Failed to add category") }
        }
    }

    fun onFormulaInputChanged(text: String) {
        val suggestions = engine?.autocompleteFormula(text) ?: emptyList()
        _uiState.update {
            it.copy(
                formulaInput = text,
                autocompleteSuggestions = suggestions,
                formulaError = null,
            )
        }
    }

    fun applyAutocompleteChip(suggestion: String) {
        val current = _uiState.value.formulaInput
        val next = if (current.isBlank()) {
            suggestion
        } else if (current.endsWith(" ") || current.endsWith("(") || current.endsWith("=")) {
            "$current$suggestion"
        } else {
            val lastSpace = current.lastIndexOfAny(charArrayOf(' ', '(', '='))
            if (lastSpace >= 0) {
                current.substring(0, lastSpace + 1) + suggestion
            } else {
                "$current $suggestion"
            }
        }
        onFormulaInputChanged("$next ")
    }

    fun submitFormula(expression: String = _uiState.value.formulaInput) {
        val engine = engine ?: return
        try {
            val entry = engine.upsertFormula(expression)
            _uiState.update {
                it.copy(
                    formulaInput = entry.expression,
                    formulaError = null,
                )
            }
            refreshFromEngine(status = "Evaluated formula: ${entry.expression} (${entry.naturalLanguage})")
            syncOpenModelSummary()
        } catch (e: Exception) {
            _uiState.update {
                it.copy(
                    formulaError = e.message ?: "Invalid formula",
                    statusMessage = "Formula error: ${e.message}",
                )
            }
        }
    }

    fun onSelectedCellEditBufferChanged(text: String) {
        _uiState.update { it.copy(selectedCellEditBuffer = text) }
    }

    fun commitSelectedCellEdit() {
        val engine = engine ?: return
        val state = _uiState.value
        if (state.selectedCellIsDerived) {
            _uiState.update {
                it.copy(statusMessage = "Derived cells are computed by formulas; edit the formula instead.")
            }
            return
        }
        val num = state.selectedCellEditBuffer.trim().toDoubleOrNull()
        if (num == null) {
            _uiState.update {
                it.copy(statusMessage = "Enter a valid numeric value")
            }
            return
        }
        try {
            engine.setCellValue(state.selectedRow.toUInt(), state.selectedCol.toUInt(), num)
            refreshFromEngine(status = "Updated ${state.selectedCellCoordinate} = $num")
        } catch (e: Exception) {
            _uiState.update {
                it.copy(statusMessage = "Edit error: ${e.message}")
            }
        }
    }

    /**
     * Rearranges the current categories onto a chosen pivot shape, derived from
     * the model's own categories (never from hardcoded names).  [rowCategoryId]
     * goes to the row axis and [columnCategoryId] to the column axis; every other
     * category falls to the page axis.  When either id is blank the first two
     * categories are used, so a caller can request a plain "swap rows/cols".
     */
    fun applyPivotLayout(rowCategoryId: String, columnCategoryId: String) {
        val engine = engine ?: return
        val all = engine.getCategories()
        if (all.isEmpty()) return
        val rowId = rowCategoryId.ifBlank { all.getOrNull(0)?.id.orEmpty() }
        val colId = columnCategoryId.ifBlank { all.getOrNull(1)?.id.orEmpty() }
        val rowCat = all.firstOrNull { it.id == rowId }
        val colCat = all.firstOrNull { it.id == colId && it.id != rowId }

        rowCat?.let { engine.pivotCategory(it.id, AxisOrientation.ROW, 0u) }
        colCat?.let { engine.pivotCategory(it.id, AxisOrientation.COLUMN, 0u) }
        all.filter { it.id != rowCat?.id && it.id != colCat?.id }
            .forEach { engine.pivotCategory(it.id, AxisOrientation.PAGE, 0u) }

        val name = deriveViewName(
            listOfNotNull(rowCat),
            listOfNotNull(colCat),
        )
        _uiState.update { it.copy(activeViewName = name) }
        refreshFromEngine(status = "Switched layout to '$name'")
    }

    fun selectAnalyticsMeasure(measureName: String) {
        val engine = engine ?: return
        val summary = engine.getSliceAnalytics(measureName)
        _uiState.update {
            it.copy(
                activeAnalyticsMeasure = summary.measureName,
                sliceAnalytics = summary,
                statusMessage = "Analyzing slice statistics for '${summary.measureName}'",
            )
        }
    }

    fun addCategoryItem(categoryIdOrName: String, itemName: String) {
        val engine = engine ?: return
        val trimmed = itemName.trim()
        if (trimmed.isEmpty()) {
            _uiState.update { it.copy(statusMessage = "Enter a non-empty item name") }
            return
        }
        try {
            val updatedTile = engine.addCategoryItem(categoryIdOrName, trimmed)
            refreshFromEngine(
                status = "Added item '$trimmed' to category '${updatedTile.name}' (${updatedTile.items.size} items)",
            )
        } catch (e: Exception) {
            _uiState.update { it.copy(statusMessage = "Add item error: ${e.message}") }
        }
    }

    fun addInputMeasure(name: String, defaultValueText: String = "0.0") {
        val engine = engine ?: return
        val trimmed = name.trim()
        if (trimmed.isEmpty()) {
            _uiState.update { it.copy(statusMessage = "Measure name is required") }
            return
        }
        val defaultVal = defaultValueText.trim().toDoubleOrNull() ?: 0.0
        try {
            val summary = engine.addInputMeasure(trimmed, defaultVal)
            refreshFromEngine(status = "Added input measure '${summary.name}' (default = $defaultVal)")
            syncOpenModelSummary()
        } catch (e: Exception) {
            _uiState.update { it.copy(statusMessage = "Add measure error: ${e.message}") }
        }
    }

    fun deleteMeasure(measureIdOrName: String) {
        val engine = engine ?: return
        try {
            engine.deleteMeasure(measureIdOrName)
            refreshFromEngine(selectedRow = 0, selectedCol = 0, status = "Deleted measure '$measureIdOrName'")
            syncOpenModelSummary()
        } catch (e: Exception) {
            _uiState.update { it.copy(statusMessage = "Delete measure error: ${e.message}") }
        }
    }

    fun applyWhatIfMultiplier(
        measureName: String,
        multiplier: Double,
        scenarioLabel: String,
    ) {
        val engine = engine ?: return
        try {
            val updatedCells = engine.applyWhatIfMultiplier(measureName, multiplier)
            refreshFromEngine(
                status = "Applied What-If '$scenarioLabel' ($measureName × $multiplier across $updatedCells cells)",
            )
        } catch (e: Exception) {
            _uiState.update { it.copy(statusMessage = "What-If error: ${e.message}") }
        }
    }

    fun onCsvBufferChanged(text: String) {
        _uiState.update { it.copy(csvImportExportBuffer = text) }
    }

    fun exportMeasureToCsvBuffer(measureName: String = _uiState.value.activeAnalyticsMeasure) {
        val engine = engine ?: return
        try {
            val csvText = engine.exportMeasureCsvText(measureName)
            _uiState.update {
                it.copy(
                    csvImportExportBuffer = csvText,
                    statusMessage = "Exported '$measureName' to CSV (${csvText.lines().count { l -> l.isNotBlank() }} rows)",
                )
            }
        } catch (e: Exception) {
            _uiState.update { it.copy(statusMessage = "CSV export error: ${e.message}") }
        }
    }

    fun importCsvFromBuffer(measureNameOverride: String = "") {
        val engine = engine ?: return
        try {
            val importedCount = engine.importCsvText(_uiState.value.csvImportExportBuffer, measureNameOverride)
            refreshFromEngine(
                status = "Imported $importedCount cells from CSV into multidimensional cube",
            )
            syncOpenModelSummary()
        } catch (e: Exception) {
            _uiState.update { it.copy(statusMessage = "CSV import error: ${e.message}") }
        }
    }

    override fun onCleared() {
        super.onCleared()
        closeEngine()
    }
}

fun ViewportSnapshot.cellAt(row: Int, col: Int): ViewportCell? {
    val cols = colHeaders.size
    if (cols <= 0 || row < 0 || col < 0 || row >= rowHeaders.size || col >= cols) return null
    val idx = row * cols + col
    return cells.getOrNull(idx)
}
