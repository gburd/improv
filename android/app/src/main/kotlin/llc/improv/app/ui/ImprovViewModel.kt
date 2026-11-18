package llc.improv.app.ui

import androidx.lifecycle.ViewModel
import llc.improv.app.DirectViewportReader
import llc.improv.app.populateDirectByteBuffer
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

data class ImprovUiState(
    val activeModelId: String = ImprovViewModel.DEFAULT_MODEL_ID,
    val activeViewName: String = "3D Financial Cube (Default)",
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
    val selectedCellCoordinate: String = "Time:Q1|Product:PixelBook|Region:Americas|Measure:Price",
    val selectedCellFormatted: String = "",
    val selectedCellEditBuffer: String = "",
    val selectedCellIsDerived: Boolean = false,
    val scrollRowIndex: Int = 0,
    val scrollColIndex: Int = 0,
    val formulaInput: String = "Margin = Revenue - COGS",
    val autocompleteSuggestions: List<String> = emptyList(),
    val formulaError: String? = null,
    val directBufferBytesWritten: Int = 0,
    val directBufferSelectedValue: Double = Double.NaN,
    val activeAnalyticsMeasure: String = "Margin",
    val sliceAnalytics: SliceAnalyticsSummary = SliceAnalyticsSummary(
        measureName = "Margin",
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
    val csvImportExportBuffer: String =
        "Time,Product,Region,TargetQuota\nQ1,PixelBook,Americas,180000\nQ2,PixelPhone,Americas,350000",
    val statusMessage: String = "Ready — 3D model (Time × Product × Region) loaded",
)

class ImprovViewModel(
    val engine: ImprovMobileEngine = ImprovMobileEngine.openOrCreate(""),
    initialModelId: String = DEFAULT_MODEL_ID,
) : ViewModel() {

    companion object {
        const val DEFAULT_MODEL_ID = "improv_sample_3d"
        const val KEY_ACTIVE_MODEL_ID = "active_model_id"
    }

    private val directViewportBuffer: ByteBuffer = ByteBuffer.allocateDirect(64 * 1024)
    private val directReader = DirectViewportReader(directViewportBuffer)

    private val _uiState = MutableStateFlow(ImprovUiState(activeModelId = initialModelId))
    val uiState: StateFlow<ImprovUiState> = _uiState.asStateFlow()

    init {
        refreshFromEngine(status = "Loaded 3D Sample Model (Time × Product × Region)")
    }

    fun setActiveModelId(modelId: String) {
        val normalized = modelId.trim().ifEmpty { DEFAULT_MODEL_ID }
        _uiState.update { it.copy(activeModelId = normalized) }
    }

    fun refreshFromEngine(
        selectedRow: Int = _uiState.value.selectedRow,
        selectedCol: Int = _uiState.value.selectedCol,
        status: String? = null,
    ) {
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

        val suggestions = engine.autocompleteFormula(_uiState.value.formulaInput)
        val targetAnalyticsMeasure = selectedCell?.measureName?.takeIf { it.isNotBlank() }
            ?: _uiState.value.activeAnalyticsMeasure.takeIf { name ->
                measures.any { it.name.equals(name, ignoreCase = true) }
            }
            ?: measures.lastOrNull()?.name
            ?: "Margin"
        val analyticsSummary = engine.getSliceAnalytics(targetAnalyticsMeasure)

        _uiState.update { current ->
            current.copy(
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
                autocompleteSuggestions = suggestions,
                directBufferBytesWritten = bytesWritten,
                directBufferSelectedValue = directVal,
                activeAnalyticsMeasure = analyticsSummary.measureName,
                sliceAnalytics = analyticsSummary,
                statusMessage = status ?: current.statusMessage,
            )
        }
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
        engine.selectPageItem(categoryId, itemId)
        refreshFromEngine(status = "Updated page slice filter")
    }

    fun addCategory(name: String, itemsCsv: String, orientation: AxisOrientation) {
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
        } catch (e: Exception) {
            _uiState.update { it.copy(formulaError = e.message ?: "Failed to add category") }
        }
    }

    fun onFormulaInputChanged(text: String) {
        val suggestions = engine.autocompleteFormula(text)
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
        try {
            val entry = engine.upsertFormula(expression)
            _uiState.update {
                it.copy(
                    formulaInput = entry.expression,
                    formulaError = null,
                )
            }
            refreshFromEngine(status = "Evaluated formula: ${entry.expression} (${entry.naturalLanguage})")
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

    fun selectViewPreset(presetName: String) {
        _uiState.update { it.copy(activeViewName = presetName) }
        when {
            presetName.contains("Product × Time", ignoreCase = true) -> {
                engine.pivotCategory("Product", AxisOrientation.ROW, 0u)
                engine.pivotCategory("Time", AxisOrientation.COLUMN, 0u)
                engine.pivotCategory("Region", AxisOrientation.PAGE, 0u)
            }
            presetName.contains("Region × Time", ignoreCase = true) -> {
                engine.pivotCategory("Region", AxisOrientation.ROW, 0u)
                engine.pivotCategory("Time", AxisOrientation.COLUMN, 0u)
                engine.pivotCategory("Product", AxisOrientation.PAGE, 0u)
            }
            else -> {
                engine.pivotCategory("Time", AxisOrientation.ROW, 0u)
                engine.pivotCategory("Product", AxisOrientation.COLUMN, 0u)
                engine.pivotCategory("Region", AxisOrientation.PAGE, 0u)
            }
        }
        refreshFromEngine(status = "Switched view to '$presetName'")
    }

    fun resetSampleModel() {
        try {
            engine.loadSampleModel()
            _uiState.update {
                it.copy(
                    activeViewName = "3D Financial Cube (Default)",
                    formulaError = null,
                )
            }
            refreshFromEngine(selectedRow = 0, selectedCol = 0, status = "Reset 3D Sample Model (Time × Product × Region)")
        } catch (e: Exception) {
            _uiState.update { it.copy(statusMessage = "Reset failed: ${e.message}") }
        }
    }

    fun createBlankWorkbook() {
        try {
            engine.createBlankModel()
            _uiState.update {
                it.copy(
                    activeViewName = "Blank Workbook (Period × Department)",
                    formulaInput = "Forecast = Budget * 1.1",
                    formulaError = null,
                )
            }
            refreshFromEngine(
                selectedRow = 0,
                selectedCol = 0,
                status = "Created blank analytical workbook (Period × Department)",
            )
        } catch (e: Exception) {
            _uiState.update { it.copy(statusMessage = "New workbook failed: ${e.message}") }
        }
    }

    fun selectAnalyticsMeasure(measureName: String) {
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
        val trimmed = name.trim()
        if (trimmed.isEmpty()) {
            _uiState.update { it.copy(statusMessage = "Measure name is required") }
            return
        }
        val defaultVal = defaultValueText.trim().toDoubleOrNull() ?: 0.0
        try {
            val summary = engine.addInputMeasure(trimmed, defaultVal)
            refreshFromEngine(status = "Added input measure '${summary.name}' (default = $defaultVal)")
        } catch (e: Exception) {
            _uiState.update { it.copy(statusMessage = "Add measure error: ${e.message}") }
        }
    }

    fun deleteMeasure(measureIdOrName: String) {
        try {
            engine.deleteMeasure(measureIdOrName)
            refreshFromEngine(selectedRow = 0, selectedCol = 0, status = "Deleted measure '$measureIdOrName'")
        } catch (e: Exception) {
            _uiState.update { it.copy(statusMessage = "Delete measure error: ${e.message}") }
        }
    }

    fun applyWhatIfMultiplier(
        measureName: String,
        multiplier: Double,
        scenarioLabel: String,
    ) {
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
        try {
            val importedCount = engine.importCsvText(_uiState.value.csvImportExportBuffer, measureNameOverride)
            refreshFromEngine(
                status = "Imported $importedCount cells from CSV into multidimensional cube",
            )
        } catch (e: Exception) {
            _uiState.update { it.copy(statusMessage = "CSV import error: ${e.message}") }
        }
    }
}

fun ViewportSnapshot.cellAt(row: Int, col: Int): ViewportCell? {
    val cols = colHeaders.size
    if (cols <= 0 || row < 0 || col < 0 || row >= rowHeaders.size || col >= cols) return null
    val idx = row * cols + col
    return cells.getOrNull(idx)
}
