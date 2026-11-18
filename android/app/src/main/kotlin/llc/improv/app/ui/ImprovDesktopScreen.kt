package llc.improv.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.PointerIcon
import androidx.compose.ui.input.pointer.pointerHoverIcon
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import uniffi.improv_android_ffi.AxisOrientation
import uniffi.improv_android_ffi.CategoryTile

/**
 * Classic NeXTSTEP-inspired Desktop / Chromebook Multi-Pane Layout (`WindowWidthSizeClass.MEDIUM` or `EXPANDED`):
 * - Top Menu Bar
 * - Direct Drag-and-Drop Category Tile Trays on the Row, Column, and Page headers
 * - Central Multidimensional Spreadsheet Grid
 * - Right-Side Model Inspector Panel
 * - Persistent Bottom Formula Pane
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ImprovDesktopScreen(
    uiState: ImprovUiState,
    viewModel: ImprovViewModel,
    modifier: Modifier = Modifier,
    onToggleLayoutOverride: (() -> Unit)? = null,
) {
    var draggedTile by remember { mutableStateOf<CategoryTile?>(null) }
    var dragPointerRoot by remember { mutableStateOf(Offset.Zero) }
    var rowTrayBounds by remember { mutableStateOf(Rect.Zero) }
    var colTrayBounds by remember { mutableStateOf(Rect.Zero) }
    var pageTrayBounds by remember { mutableStateOf(Rect.Zero) }

    var newCategoryName by remember { mutableStateOf("") }
    var newCategoryItems by remember { mutableStateOf("Online, Retail") }

    val handleDragStart: (CategoryTile, Offset) -> Unit = { tile, startRoot ->
        draggedTile = tile
        dragPointerRoot = startRoot
    }
    val handleDragDelta: (Offset) -> Unit = { delta ->
        dragPointerRoot += delta
    }
    val handleDragEnd: () -> Unit = {
        val tile = draggedTile
        if (tile != null) {
            when {
                rowTrayBounds.contains(dragPointerRoot) ->
                    viewModel.pivotCategory(tile.id, AxisOrientation.ROW, 0)
                colTrayBounds.contains(dragPointerRoot) ->
                    viewModel.pivotCategory(tile.id, AxisOrientation.COLUMN, 0)
                pageTrayBounds.contains(dragPointerRoot) ->
                    viewModel.pivotCategory(tile.id, AxisOrientation.PAGE, 0)
            }
        }
        draggedTile = null
    }

    Column(
        modifier = modifier
            .fillMaxSize()
            .background(NextPlatinumBg)
            .semantics { contentDescription = "Improv Desktop NeXTSTEP Multi-Pane Screen" },
    ) {
        // 1. NeXTSTEP Top Menu Bar
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .background(NextDarkChrome)
                .border(1.dp, NextBorderDark)
                .padding(horizontal = 12.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Box(
                    modifier = Modifier
                        .background(NextAccentAmber, RoundedCornerShape(3.dp))
                        .padding(horizontal = 8.dp, vertical = 3.dp),
                ) {
                    Text(
                        text = "NeXT / IMPROV",
                        color = Color.Black,
                        fontSize = 11.sp,
                        fontWeight = FontWeight.ExtraBold,
                        fontFamily = FontFamily.Monospace,
                    )
                }
                Text(
                    text = "View: ${uiState.activeViewName}",
                    color = Color.White,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.SemiBold,
                )
                listOf(
                    "Time × Product",
                    "Product × Time",
                    "Region × Time",
                ).forEach { preset ->
                    Box(
                        modifier = Modifier
                            .pointerHoverIcon(PointerIcon.Hand)
                            .semantics { contentDescription = "Switch to view preset $preset" }
                            .background(Color(0xFF334155), RoundedCornerShape(3.dp))
                            .clickable { viewModel.selectViewPreset(preset) }
                            .padding(horizontal = 8.dp, vertical = 3.dp),
                    ) {
                        Text(
                            text = preset,
                            color = Color(0xFFE2E8F0),
                            fontSize = 11.sp,
                            fontFamily = FontFamily.Monospace,
                        )
                    }
                }
            }

            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    text = "Direct ByteBuffer: ${uiState.directBufferBytesWritten} B",
                    color = Color(0xFF86EFAC),
                    fontSize = 11.sp,
                    fontFamily = FontFamily.Monospace,
                )
                OutlinedButton(
                    onClick = { viewModel.createBlankWorkbook() },
                    contentPadding = PaddingValues(horizontal = 8.dp, vertical = 3.dp),
                    modifier = Modifier
                        .height(28.dp)
                        .pointerHoverIcon(PointerIcon.Hand)
                        .semantics { contentDescription = "Create Blank Workbook" },
                ) {
                    Text("Blank Workbook", color = Color(0xFFFDE68A), fontSize = 11.sp)
                }
                OutlinedButton(
                    onClick = { viewModel.resetSampleModel() },
                    contentPadding = PaddingValues(horizontal = 10.dp, vertical = 3.dp),
                    modifier = Modifier
                        .height(28.dp)
                        .pointerHoverIcon(PointerIcon.Hand)
                        .semantics { contentDescription = "Reset 3D Sample Model" },
                ) {
                    Text("Reset 3D Model", color = Color.White, fontSize = 11.sp)
                }
                if (onToggleLayoutOverride != null) {
                    OutlinedButton(
                        onClick = onToggleLayoutOverride,
                        contentPadding = PaddingValues(horizontal = 8.dp, vertical = 3.dp),
                        modifier = Modifier
                            .height(28.dp)
                            .pointerHoverIcon(PointerIcon.Hand),
                    ) {
                        Text("Phone View", color = Color(0xFFCBD5E1), fontSize = 11.sp)
                    }
                }
            }
        }

        // 2. Drag-and-Drop Category Tile Trays (Row Tray, Column Tray, Page Tray)
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .background(NextMidChrome)
                .padding(horizontal = 10.dp, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            // Row Category Tray
            CategoryDropTray(
                title = "ROW TRAY",
                accentColor = Color(0xFF1D4ED8),
                tiles = uiState.rowCategories,
                isDropHighlighted = draggedTile != null && rowTrayBounds.contains(dragPointerRoot),
                onPivotTo = { tile, axis -> viewModel.pivotCategory(tile.id, axis, 0) },
                onCycleAxis = viewModel::cycleCategoryAxis,
                onDragStartRoot = handleDragStart,
                onDragDelta = handleDragDelta,
                onDragEnd = handleDragEnd,
                modifier = Modifier
                    .weight(1f)
                    .onGloballyPositioned { rowTrayBounds = it.boundsInRoot() }
                    .semantics { contentDescription = "Row category tile tray" },
            )

            // Column Category Tray
            CategoryDropTray(
                title = "COLUMN TRAY",
                accentColor = Color(0xFF047857),
                tiles = uiState.columnCategories,
                isDropHighlighted = draggedTile != null && colTrayBounds.contains(dragPointerRoot),
                onPivotTo = { tile, axis -> viewModel.pivotCategory(tile.id, axis, 0) },
                onCycleAxis = viewModel::cycleCategoryAxis,
                onDragStartRoot = handleDragStart,
                onDragDelta = handleDragDelta,
                onDragEnd = handleDragEnd,
                modifier = Modifier
                    .weight(1f)
                    .onGloballyPositioned { colTrayBounds = it.boundsInRoot() }
                    .semantics { contentDescription = "Column category tile tray" },
            )

            // Page Category Tray
            CategoryDropTray(
                title = "PAGE TRAY",
                accentColor = Color(0xFFB45309),
                tiles = uiState.pageCategories,
                isDropHighlighted = draggedTile != null && pageTrayBounds.contains(dragPointerRoot),
                onPivotTo = { tile, axis -> viewModel.pivotCategory(tile.id, axis, 0) },
                onCycleAxis = viewModel::cycleCategoryAxis,
                onDragStartRoot = handleDragStart,
                onDragDelta = handleDragDelta,
                onDragEnd = handleDragEnd,
                modifier = Modifier
                    .weight(1f)
                    .onGloballyPositioned { pageTrayBounds = it.boundsInRoot() }
                    .semantics { contentDescription = "Page category tile tray" },
            )
        }

        // Page Filter Strip + Semantic Coordinate Bar
        PageCategoryFilterStrip(
            pageCategories = uiState.pageCategories,
            onSelectPageItem = viewModel::selectPageItem,
        )
        SemanticCoordinateBar(
            uiState = uiState,
            onEditBufferChanged = viewModel::onSelectedCellEditBufferChanged,
            onCommitCellEdit = viewModel::commitSelectedCellEdit,
        )

        // 3. Central Workspace: Left/Center Spreadsheet Grid + Right Model Inspector Panel
        Row(
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .padding(6.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            // Central Spreadsheet Grid
            SpreadsheetGridViewport(
                uiState = uiState,
                onSelectCell = viewModel::selectCell,
                onMoveFocus = viewModel::moveCellFocus,
                onTabFocus = viewModel::moveTabFocus,
                onScrollChanged = viewModel::updateScrollIndices,
                modifier = Modifier
                    .weight(1f)
                    .fillMaxHeight(),
            )

            // Right-Side Model Inspector Panel
            Surface(
                modifier = Modifier
                    .width(290.dp)
                    .fillMaxHeight()
                    .border(1.dp, NextBorderDark, RoundedCornerShape(4.dp))
                    .semantics { contentDescription = "Model Inspector Panel" },
                color = NextPanelSurface,
                shape = RoundedCornerShape(4.dp),
            ) {
                Column(
                    modifier = Modifier
                        .fillMaxSize()
                        .padding(10.dp)
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Box(
                        modifier = Modifier
                            .fillMaxWidth()
                            .background(NextDarkChrome, RoundedCornerShape(3.dp))
                            .padding(horizontal = 8.dp, vertical = 5.dp),
                    ) {
                        Text(
                            text = "MODEL INSPECTOR",
                            color = Color.White,
                            fontSize = 11.sp,
                            fontWeight = FontWeight.Bold,
                            fontFamily = FontFamily.Monospace,
                        )
                    }

                    Text(
                        text = "Selected Semantic Coordinate",
                        fontSize = 11.sp,
                        fontWeight = FontWeight.Bold,
                        color = Color(0xFF334155),
                    )
                    Text(
                        text = uiState.selectedCellCoordinate,
                        fontSize = 11.sp,
                        fontFamily = FontFamily.Monospace,
                        color = Color(0xFF1E3A8A),
                    )

                    Text(
                        text = "Viewport Scroll: row=${uiState.scrollRowIndex}, col=${uiState.scrollColIndex}",
                        fontSize = 11.sp,
                        fontFamily = FontFamily.Monospace,
                        color = Color(0xFF475569),
                    )
                    Text(
                        text = "DirectBuffer Cell f64: ${
                            if (uiState.directBufferSelectedValue.isNaN()) "NaN"
                            else String.format(java.util.Locale.US, "%.2f", uiState.directBufferSelectedValue)
                        }",
                        fontSize = 11.sp,
                        fontFamily = FontFamily.Monospace,
                        color = Color(0xFF047857),
                    )

                    HorizontalDivider()
                    Text(
                        text = "Measures (${uiState.measures.size})",
                        fontSize = 12.sp,
                        fontWeight = FontWeight.Bold,
                        color = Color(0xFF0F172A),
                    )
                    uiState.measures.forEach { m ->
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .background(
                                    if (m.isDerived) Color(0xFFEFF6FF) else Color.White,
                                    RoundedCornerShape(3.dp),
                                )
                                .border(0.5.dp, Color(0xFFCBD5E1), RoundedCornerShape(3.dp))
                                .padding(horizontal = 8.dp, vertical = 4.dp),
                            horizontalArrangement = Arrangement.SpaceBetween,
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(
                                text = if (m.isDerived) "ƒ ${m.name}" else m.name,
                                fontSize = 11.sp,
                                fontWeight = FontWeight.SemiBold,
                                color = if (m.isDerived) Color(0xFF1D4ED8) else Color(0xFF0F172A),
                            )
                            Text(
                                text = m.formulaText ?: "Input",
                                fontSize = 10.sp,
                                fontFamily = FontFamily.Monospace,
                                color = Color(0xFF64748B),
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }

                    HorizontalDivider()
                    Text(
                        text = "Add Category Dimension",
                        fontSize = 12.sp,
                        fontWeight = FontWeight.Bold,
                        color = Color(0xFF0F172A),
                    )
                    OutlinedTextField(
                        value = newCategoryName,
                        onValueChange = { newCategoryName = it },
                        label = { Text("Category Name", fontSize = 11.sp) },
                        singleLine = true,
                        modifier = Modifier
                            .fillMaxWidth()
                            .pointerHoverIcon(PointerIcon.Text),
                    )
                    OutlinedTextField(
                        value = newCategoryItems,
                        onValueChange = { newCategoryItems = it },
                        label = { Text("Items (comma-separated)", fontSize = 11.sp) },
                        singleLine = true,
                        modifier = Modifier
                            .fillMaxWidth()
                            .pointerHoverIcon(PointerIcon.Text),
                    )
                    Button(
                        onClick = {
                            viewModel.addCategory(
                                name = newCategoryName,
                                itemsCsv = newCategoryItems,
                                orientation = AxisOrientation.PAGE,
                            )
                            newCategoryName = ""
                        },
                        modifier = Modifier
                            .fillMaxWidth()
                            .pointerHoverIcon(PointerIcon.Hand),
                        colors = ButtonDefaults.buttonColors(containerColor = NextDarkChrome),
                    ) {
                        Text("Add Category to Page Tray", fontSize = 11.sp)
                    }

                    HorizontalDivider()
                    AnalyticalStudioSection(
                        uiState = uiState,
                        viewModel = viewModel,
                    )
                }
            }
        }

        // 4. Persistent Bottom Formula Pane
        Surface(
            modifier = Modifier
                .fillMaxWidth()
                .border(1.dp, NextBorderDark)
                .semantics { contentDescription = "Persistent Bottom Formula Pane" },
            color = NextPanelSurface,
        ) {
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp, vertical = 8.dp),
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        text = "FORMULA PANE (Symbolic DSL & Controlled Natural Language)",
                        fontSize = 11.sp,
                        fontWeight = FontWeight.Bold,
                        fontFamily = FontFamily.Monospace,
                        color = Color(0xFF1E293B),
                    )
                    if (draggedTile != null) {
                        Text(
                            text = "Dragging '${draggedTile?.name}' — drop on ROW, COLUMN, or PAGE tray",
                            fontSize = 11.sp,
                            fontWeight = FontWeight.Bold,
                            color = NextAccentAmber,
                        )
                    }
                }
                Spacer(modifier = Modifier.height(4.dp))
                FormulaEditorSection(
                    formulaInput = uiState.formulaInput,
                    autocompleteSuggestions = uiState.autocompleteSuggestions,
                    formulas = uiState.formulas,
                    formulaError = uiState.formulaError,
                    onFormulaInputChanged = viewModel::onFormulaInputChanged,
                    onApplySuggestion = viewModel::applyAutocompleteChip,
                    onSubmitFormula = { viewModel.submitFormula() },
                    onSelectExistingFormula = { entry ->
                        viewModel.onFormulaInputChanged(entry.expression)
                    },
                )
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun CategoryDropTray(
    title: String,
    accentColor: Color,
    tiles: List<CategoryTile>,
    isDropHighlighted: Boolean,
    onPivotTo: (CategoryTile, AxisOrientation) -> Unit,
    onCycleAxis: (CategoryTile) -> Unit,
    onDragStartRoot: (CategoryTile, Offset) -> Unit,
    onDragDelta: (Offset) -> Unit,
    onDragEnd: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val borderColor = if (isDropHighlighted) Color(0xFFFBBF24) else Color(0xFF64748B)
    val borderWidth = if (isDropHighlighted) 2.dp else 1.dp
    Column(
        modifier = modifier
            .background(Color(0xFF1E242B), RoundedCornerShape(4.dp))
            .border(borderWidth, borderColor, RoundedCornerShape(4.dp))
            .padding(horizontal = 8.dp, vertical = 6.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Box(
                modifier = Modifier
                    .background(accentColor, RoundedCornerShape(2.dp))
                    .padding(horizontal = 5.dp, vertical = 1.dp),
            ) {
                Text(
                    text = title,
                    color = Color.White,
                    fontSize = 9.sp,
                    fontWeight = FontWeight.Bold,
                    fontFamily = FontFamily.Monospace,
                )
            }
            Text(
                text = "Drag or click R/C/P",
                color = Color(0xFF94A3B8),
                fontSize = 10.sp,
            )
        }
        if (tiles.isEmpty()) {
            Text(
                text = "(Drop category tile here)",
                color = Color(0xFF64748B),
                fontSize = 11.sp,
                fontFamily = FontFamily.Monospace,
                modifier = Modifier.padding(vertical = 4.dp),
            )
        } else {
            FlowRow(
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                tiles.forEach { tile ->
                    CategoryTileChip(
                        tile = tile,
                        onPivotTo = { axis -> onPivotTo(tile, axis) },
                        onCycleAxis = { onCycleAxis(tile) },
                        onDragStartRoot = onDragStartRoot,
                        onDragDelta = onDragDelta,
                        onDragEnd = onDragEnd,
                    )
                }
            }
        }
    }
}
