package llc.improv.app.ui

import llc.improv.app.DirectViewportReader
import llc.improv.app.populateDirectByteBuffer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.improv_android_ffi.AxisOrientation
import java.nio.ByteBuffer

class ImprovViewModelTest {

    @Test
    fun sampleModelInitializesCategoriesFormulasAndDirectBuffer() {
        val vm = ImprovViewModel()
        val state = vm.uiState.value

        assertEquals(1, state.rowCategories.size)
        assertEquals("Time", state.rowCategories.first().name)
        assertEquals(1, state.columnCategories.size)
        assertEquals("Product", state.columnCategories.first().name)
        assertEquals(1, state.pageCategories.size)
        assertEquals("Region", state.pageCategories.first().name)

        assertEquals(6, state.measures.size)
        assertEquals(3, state.formulas.size)
        assertTrue(state.selectedCellCoordinate.contains("Time:Q1"))
        assertTrue(state.selectedCellCoordinate.contains("Product:PixelBook"))
        assertTrue(state.selectedCellCoordinate.contains("Region:Americas"))
        assertTrue(state.selectedCellCoordinate.contains("Measure:Price"))
        assertTrue(state.directBufferBytesWritten > 16)
        assertFalse(state.directBufferSelectedValue.isNaN())
    }

    @Test
    fun pivotCategoryCycleAxisAndPageSelection() {
        val vm = ImprovViewModel()

        // Pivot Region to Row axis
        vm.pivotCategory("Region", AxisOrientation.ROW, 1)
        assertEquals(2, vm.uiState.value.rowCategories.size)
        assertEquals(0, vm.uiState.value.pageCategories.size)

        // Cycle Time axis from Row -> Column -> Page -> Row
        val timeTile = vm.uiState.value.rowCategories.first { it.name == "Time" }
        vm.cycleCategoryAxis(timeTile)
        assertTrue(vm.uiState.value.columnCategories.any { it.name == "Time" })

        val timeColTile = vm.uiState.value.columnCategories.first { it.name == "Time" }
        vm.cycleCategoryAxis(timeColTile)
        assertTrue(vm.uiState.value.pageCategories.any { it.name == "Time" })

        // Select page item on Time
        val timePageTile = vm.uiState.value.pageCategories.first { it.name == "Time" }
        val secondItem = timePageTile.items[1]
        vm.selectPageItem(timePageTile.id, secondItem.id)
        assertEquals(
            secondItem.id,
            vm.uiState.value.pageCategories.first { it.name == "Time" }.selectedPageItemId
        )

        // Add new category dimension
        vm.addCategory("Channel", "Online, Retail", AxisOrientation.PAGE)
        assertTrue(vm.uiState.value.pageCategories.any { it.name == "Channel" })
    }

    @Test
    fun formulasAutocompleteCellEditingViewPresetsAndReset() {
        val vm = ImprovViewModel()

        // Arrow-key and Tab cell focus navigation + scroll index tracking
        vm.moveCellFocus(0, 1)
        assertEquals(0, vm.uiState.value.selectedRow)
        assertEquals(1, vm.uiState.value.selectedCol)

        vm.moveTabFocus(reverse = false)
        assertEquals(0, vm.uiState.value.selectedRow)
        assertEquals(2, vm.uiState.value.selectedCol)

        vm.moveTabFocus(reverse = true)
        assertEquals(0, vm.uiState.value.selectedRow)
        assertEquals(1, vm.uiState.value.selectedCol)

        vm.updateScrollIndices(3, 1)
        assertEquals(3, vm.uiState.value.scrollRowIndex)
        assertEquals(1, vm.uiState.value.scrollColIndex)

        // Edit input cell (row 0 is Q1 · Price, an input measure)
        vm.selectCell(0, 0)
        assertFalse(vm.uiState.value.selectedCellIsDerived)
        vm.onSelectedCellEditBufferChanged("1499.0")
        vm.commitSelectedCellEdit()
        assertEquals(1499.0, vm.uiState.value.directBufferSelectedValue, 0.01)

        // Select a derived cell (row 3 is Q1 · Revenue) and verify derived protection
        vm.selectCell(3, 0)
        assertTrue(vm.uiState.value.selectedCellIsDerived)
        vm.onSelectedCellEditBufferChanged("999")
        vm.commitSelectedCellEdit()

        // Autocomplete and natural-language formula submission
        vm.onFormulaInputChanged("NetProfit equals ")
        vm.applyAutocompleteChip("Margin")
        vm.onFormulaInputChanged("NetProfit equals Margin minus UnitCost")
        assertTrue(vm.uiState.value.autocompleteSuggestions.isNotEmpty())
        vm.submitFormula()

        assertEquals(null, vm.uiState.value.formulaError)
        assertEquals(4, vm.uiState.value.formulas.size)
        assertTrue(vm.uiState.value.formulas.any { it.measureName == "NetProfit" })

        // Switch view presets
        vm.selectViewPreset("Product × Time")
        assertEquals("Product × Time", vm.uiState.value.activeViewName)
        vm.selectViewPreset("Region × Time")
        assertEquals("Region × Time", vm.uiState.value.activeViewName)
        vm.selectViewPreset("Time × Product")
        assertEquals("Time × Product", vm.uiState.value.activeViewName)

        // Reset sample model
        vm.resetSampleModel()
        assertEquals(3, vm.uiState.value.formulas.size)
    }

    @Test
    fun directViewportReaderReadsBinaryHeaderAndCells() {
        val vm = ImprovViewModel()
        val buf = ByteBuffer.allocateDirect(8192)
        val written = vm.engine.populateDirectByteBuffer(buf, 0u, 10u, 0u, 3u)
        assertTrue(written > 16)

        val reader = DirectViewportReader(buf)
        assertEquals(10, reader.rows)
        assertEquals(3, reader.cols)
        assertEquals(24, reader.totalRows)
        assertEquals(3, reader.totalCols)
        assertFalse(reader.numericValue(0, 0).isNaN())
        assertTrue(reader.measureId(0, 0) >= 100)
        assertTrue(reader.rowItemId(0, 0) > 0)
        assertTrue(reader.colItemId(0, 0) > 0)
        assertEquals(1.toByte(), reader.cellTag(0, 0))
        assertFalse(reader.isDerived(0, 0))
        assertTrue(reader.isDerived(3, 0))
        assertNotNull(vm.uiState.value.viewport.cellAt(0, 0))
    }

    @Test
    fun analyticalStudioSliceStatsWhatIfShocksSchemaAuthoringAndCsvRoundTrip() {
        val vm = ImprovViewModel()

        // 1. Slice Analytics on Margin
        vm.selectAnalyticsMeasure("Margin")
        val marginStats = vm.uiState.value.sliceAnalytics
        assertEquals("Margin", marginStats.measureName)
        assertEquals(12u, marginStats.cellCount)
        assertTrue(marginStats.sum > 0.0)
        assertTrue(marginStats.max >= marginStats.min)
        assertEquals(4, marginStats.seriesValues.size)

        // 2. What-If Scenario Shock (+10% Price Lift)
        val initialRevenueSum = vm.engine.getSliceAnalytics("Revenue").sum
        vm.applyWhatIfMultiplier("Price", 1.10, "+10% Price Lift")
        val shockedRevenueSum = vm.engine.getSliceAnalytics("Revenue").sum
        assertTrue(shockedRevenueSum > initialRevenueSum)

        // 3. Dynamic Schema Authoring: add item to Time and add/delete an input measure
        vm.addCategoryItem("Time", "Q5")
        assertEquals(5, vm.uiState.value.rowCategories.first { it.name == "Time" }.items.size)

        vm.addInputMeasure("PromoSpend", "500.0")
        assertTrue(vm.uiState.value.measures.any { it.name == "PromoSpend" })
        vm.deleteMeasure("PromoSpend")
        assertFalse(vm.uiState.value.measures.any { it.name == "PromoSpend" })

        // 4. CSV Export and Import into the Multidimensional Cube
        vm.exportMeasureToCsvBuffer("Revenue")
        assertTrue(vm.uiState.value.csvImportExportBuffer.contains("Time,Product,Region,Revenue"))

        vm.onCsvBufferChanged(
            "Time,Product,Region,TargetQuota\nQ1,PixelBook,Americas,220000\nQ2,PixelPhone,EMEA,410000"
        )
        vm.importCsvFromBuffer()
        assertTrue(vm.uiState.value.measures.any { it.name == "TargetQuota" })

        // 5. Create Blank Workbook
        vm.createBlankWorkbook()
        assertEquals("Blank Workbook (Period × Department)", vm.uiState.value.activeViewName)
        assertTrue(vm.uiState.value.measures.any { it.name == "Budget" })
    }
}
