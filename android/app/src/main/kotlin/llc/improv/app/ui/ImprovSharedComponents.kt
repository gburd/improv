package llc.improv.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AssistChip
import androidx.compose.material3.AssistChipDefaults
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.isShiftPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.PointerIcon
import androidx.compose.ui.input.pointer.pointerHoverIcon
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import uniffi.improv_android_ffi.AxisOrientation
import uniffi.improv_android_ffi.CategoryTile
import uniffi.improv_android_ffi.FormulaEntry
// Classic NeXTSTEP & Lotus Improv palette
val NextDarkChrome = Color(0xFF23272E)
val NextMidChrome = Color(0xFF343A44)
val NextPlatinumBg = Color(0xFFECEAE4)
val NextPanelSurface = Color(0xFFF6F5F0)
val NextBorderDark = Color(0xFF1F2329)
val NextBorderLight = Color(0xFFFFFFFF)
val NextAccentAmber = Color(0xFFD97706)
val NextAccentBlue = Color(0xFF1D4ED8)
val NextDerivedCellBg = Color(0xFFEFF6FF)
val NextInputCellBg = Color(0xFFFFFFFF)
val NextSelectedCellBg = Color(0xFFDBEAFE)

/**
 * Interactive Category Tile with `PointerIcon.Hand` hover icon, accessibility semantics,
 * optional drag-and-drop gesture callbacks, and axis pivot controls.
 */
@Composable
fun CategoryTileChip(
    tile: CategoryTile,
    onPivotTo: (AxisOrientation) -> Unit,
    onCycleAxis: () -> Unit,
    modifier: Modifier = Modifier,
    onDragStartRoot: ((CategoryTile, Offset) -> Unit)? = null,
    onDragDelta: ((Offset) -> Unit)? = null,
    onDragEnd: (() -> Unit)? = null,
) {
    var rootPosition by remember { mutableStateOf(Offset.Zero) }
    val badgeColor = when (tile.orientation) {
        AxisOrientation.ROW -> Color(0xFF1D4ED8)
        AxisOrientation.COLUMN -> Color(0xFF047857)
        AxisOrientation.PAGE -> Color(0xFFB45309)
    }

    Surface(
        modifier = modifier
            .onGloballyPositioned { coords -> rootPosition = coords.positionInRoot() }
            .pointerHoverIcon(PointerIcon.Hand)
            .semantics {
                contentDescription =
                    "Category tile ${tile.name} on ${tile.orientation.name} axis (${tile.items.size} items)"
            }
            .then(
                if (onDragStartRoot != null && onDragDelta != null && onDragEnd != null) {
                    Modifier.pointerInput(tile.id, tile.orientation) {
                        detectDragGestures(
                            onDragStart = { localOffset ->
                                onDragStartRoot(tile, rootPosition + localOffset)
                            },
                            onDrag = { change, dragAmount ->
                                change.consume()
                                onDragDelta(dragAmount)
                            },
                            onDragEnd = { onDragEnd() },
                            onDragCancel = { onDragEnd() },
                        )
                    }
                } else {
                    Modifier
                }
            )
            .clickable { onCycleAxis() },
        shape = RoundedCornerShape(4.dp),
        color = Color(0xFF2E3440),
        tonalElevation = 2.dp,
    ) {
        Row(
            modifier = Modifier
                .border(1.dp, Color(0xFF64748B), RoundedCornerShape(4.dp))
                .padding(horizontal = 8.dp, vertical = 5.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Box(
                modifier = Modifier
                    .background(badgeColor, RoundedCornerShape(3.dp))
                    .padding(horizontal = 5.dp, vertical = 2.dp),
            ) {
                Text(
                    text = tile.orientation.name.first().toString(),
                    color = Color.White,
                    fontSize = 10.sp,
                    fontWeight = FontWeight.Bold,
                    fontFamily = FontFamily.Monospace,
                )
            }
            Text(
                text = tile.name,
                color = Color.White,
                fontSize = 12.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = "(${tile.items.size})",
                color = Color(0xFFCBD5E1),
                fontSize = 11.sp,
                fontFamily = FontFamily.Monospace,
            )
            // Compact axis quick-switch pills (R / C / P)
            Row(horizontalArrangement = Arrangement.spacedBy(2.dp)) {
                AxisOrientation.entries.forEach { axis ->
                    val isCurrent = tile.orientation == axis
                    Box(
                        modifier = Modifier
                            .pointerHoverIcon(PointerIcon.Hand)
                            .semantics {
                                contentDescription = "Pivot ${tile.name} to ${axis.name}"
                            }
                            .background(
                                if (isCurrent) Color(0xFF94A3B8) else Color(0xFF475569),
                                RoundedCornerShape(2.dp),
                            )
                            .clickable { onPivotTo(axis) }
                            .padding(horizontal = 4.dp, vertical = 1.dp),
                    ) {
                        Text(
                            text = axis.name.first().toString(),
                            color = if (isCurrent) Color(0xFF0F172A) else Color(0xFFE2E8F0),
                            fontSize = 9.sp,
                            fontWeight = FontWeight.Bold,
                            fontFamily = FontFamily.Monospace,
                        )
                    }
                }
            }
        }
    }
}

/**
 * 2D Multidimensional Spreadsheet Grid with sticky row/column headers, keyboard arrow-key
 * and Tab cell navigation, scroll index tracking, and accessibility semantics.
 */
@Composable
fun SpreadsheetGridViewport(
    uiState: ImprovUiState,
    onSelectCell: (Int, Int) -> Unit,
    onMoveFocus: (Int, Int) -> Unit,
    onTabFocus: (Boolean) -> Unit,
    onScrollChanged: (Int, Int) -> Unit,
    modifier: Modifier = Modifier,
) {
    val verticalScroll = rememberScrollState()
    val horizontalScroll = rememberScrollState()
    val gridFocusRequester = remember { FocusRequester() }

    LaunchedEffect(uiState.scrollRowIndex, uiState.scrollColIndex) {
        val targetRowPx = (uiState.scrollRowIndex * 36).coerceAtLeast(0)
        val targetColPx = (uiState.scrollColIndex * 120).coerceAtLeast(0)
        if (!verticalScroll.isScrollInProgress && (verticalScroll.value / 36) != uiState.scrollRowIndex) {
            verticalScroll.scrollTo(targetRowPx)
        }
        if (!horizontalScroll.isScrollInProgress && (horizontalScroll.value / 120) != uiState.scrollColIndex) {
            horizontalScroll.scrollTo(targetColPx)
        }
    }

    LaunchedEffect(verticalScroll, horizontalScroll) {
        androidx.compose.runtime.snapshotFlow {
            Triple(
                verticalScroll.value / 36,
                horizontalScroll.value / 120,
                verticalScroll.isScrollInProgress || horizontalScroll.isScrollInProgress,
            )
        }.collect { (r, c, isUserScrolling) ->
            if (isUserScrolling) {
                onScrollChanged(r, c)
            }
        }
    }

    val vp = uiState.viewport
    val rowHeaders = vp.rowHeaders
    val colHeaders = vp.colHeaders

    val rowHeaderWidth = 176.dp
    val cellWidth = 124.dp
    val cellHeight = 36.dp

    Column(
        modifier = modifier
            .focusRequester(gridFocusRequester)
            .focusable()
            .semantics {
                contentDescription =
                    "Multidimensional spreadsheet grid (${rowHeaders.size} rows by ${colHeaders.size} columns)"
            }
            .onPreviewKeyEvent { event ->
                if (event.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
                when (event.key) {
                    Key.DirectionUp -> {
                        onMoveFocus(-1, 0)
                        true
                    }
                    Key.DirectionDown -> {
                        onMoveFocus(1, 0)
                        true
                    }
                    Key.DirectionLeft -> {
                        onMoveFocus(0, -1)
                        true
                    }
                    Key.DirectionRight -> {
                        onMoveFocus(0, 1)
                        true
                    }
                    Key.Tab -> {
                        onTabFocus(event.isShiftPressed)
                        true
                    }
                    else -> false
                }
            }
            .border(1.dp, Color(0xFF64748B))
            .background(Color.White),
    ) {
        // Sticky Top Column Header Row
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .background( Color(0xFFE2E8F0))
                .border(0.5.dp, Color(0xFF94A3B8)),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                modifier = Modifier
                    .width(rowHeaderWidth)
                    .height(cellHeight)
                    .background(Color(0xFFCBD5E1))
                    .border(0.5.dp, Color(0xFF64748B))
                    .padding(horizontal = 8.dp),
                contentAlignment = Alignment.CenterStart,
            ) {
                val rowAxisNames = uiState.rowCategories.joinToString("/") { it.name }.ifEmpty { "Row" }
                val colAxisNames = uiState.columnCategories.joinToString("/") { it.name }.ifEmpty { "Col" }
                Text(
                    text = "$rowAxisNames · Measure \\ $colAxisNames",
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Bold,
                    color = Color(0xFF1E293B),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Row(
                modifier = Modifier.horizontalScroll(horizontalScroll),
            ) {
                colHeaders.forEachIndexed { colIdx, colTitle ->
                    val isColSelected = colIdx == uiState.selectedCol
                    Box(
                        modifier = Modifier
                            .width(cellWidth)
                            .height(cellHeight)
                            .background(
                                if (isColSelected) Color(0xFFBFDBFE) else Color(0xFFE2E8F0)
                            )
                            .border(0.5.dp, Color(0xFF94A3B8))
                            .padding(horizontal = 8.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            text = colTitle,
                            fontSize = 12.sp,
                            fontWeight = FontWeight.Bold,
                            color = Color(0xFF0F172A),
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
            }
        }

        // Scrollable Grid Body
        Column(
            modifier = Modifier
                .weight(1f, fill = false)
                .verticalScroll(verticalScroll),
        ) {
            rowHeaders.forEachIndexed { rowIdx, rowTitle ->
                val isRowSelected = rowIdx == uiState.selectedRow
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    // Sticky Row Header
                    Box(
                        modifier = Modifier
                            .width(rowHeaderWidth)
                            .height(cellHeight)
                            .background(
                                if (isRowSelected) Color(0xFFDBEAFE) else Color(0xFFF1F5F9)
                            )
                            .border(0.5.dp, Color(0xFFCBD5E1))
                            .padding(horizontal = 8.dp),
                        contentAlignment = Alignment.CenterStart,
                    ) {
                        Text(
                            text = rowTitle,
                            fontSize = 11.sp,
                            fontWeight = if (isRowSelected) FontWeight.Bold else FontWeight.Medium,
                            color = Color(0xFF1E293B),
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }

                    // Cells in Row
                    Row(
                        modifier = Modifier.horizontalScroll(horizontalScroll),
                    ) {
                        colHeaders.forEachIndexed { colIdx, colTitle ->
                            val cell = vp.cellAt(rowIdx, colIdx)
                            val isSelected =
                                rowIdx == uiState.selectedRow && colIdx == uiState.selectedCol
                            val isDerived = cell?.isDerived ?: false
                            val bg = when {
                                isSelected -> NextSelectedCellBg
                                isDerived -> NextDerivedCellBg
                                else -> NextInputCellBg
                            }
                            val borderColor = if (isSelected) NextAccentBlue else Color(0xFFE2E8F0)
                            val borderWidth = if (isSelected) 2.dp else 0.5.dp
                            val textValue = cell?.formatted?.ifEmpty { "—" } ?: "—"

                            Box(
                                modifier = Modifier
                                    .width(cellWidth)
                                    .height(cellHeight)
                                    .background(bg)
                                    .border(borderWidth, borderColor)
                                    .semantics {
                                        contentDescription =
                                            "Cell $rowTitle, $colTitle: $textValue (${if (isDerived) "derived" else "input"})"
                                    }
                                    .clickable {
                                        gridFocusRequester.requestFocus()
                                        onSelectCell(rowIdx, colIdx)
                                    }
                                    .padding(horizontal = 8.dp),
                                contentAlignment = Alignment.CenterEnd,
                            ) {
                                Row(
                                    modifier = Modifier.fillMaxWidth(),
                                    horizontalArrangement = Arrangement.SpaceBetween,
                                    verticalAlignment = Alignment.CenterVertically,
                                ) {
                                    Text(
                                        text = if (isDerived) "ƒ" else "",
                                        fontSize = 9.sp,
                                        color = Color(0xFF3B82F6),
                                        fontWeight = FontWeight.Bold,
                                    )
                                    Text(
                                        text = textValue,
                                        fontSize = 12.sp,
                                        fontFamily = FontFamily.Monospace,
                                        fontWeight = if (isDerived) FontWeight.SemiBold else FontWeight.Normal,
                                        color = if (isDerived) Color(0xFF1E3A8A) else Color(0xFF0F172A),
                                        textAlign = TextAlign.End,
                                        maxLines = 1,
                                    )
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/**
 * Formula Editor with `PointerIcon.Text` on the text field, autocomplete suggestion chips,
 * and numbered Lotus Improv formula entries.
 */
@Composable
fun FormulaEditorSection(
    formulaInput: String,
    autocompleteSuggestions: List<String>,
    formulas: List<FormulaEntry>,
    formulaError: String?,
    onFormulaInputChanged: (String) -> Unit,
    onApplySuggestion: (String) -> Unit,
    onSubmitFormula: () -> Unit,
    onSelectExistingFormula: (FormulaEntry) -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier = modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            OutlinedTextField(
                value = formulaInput,
                onValueChange = onFormulaInputChanged,
                modifier = Modifier
                    .weight(1f)
                    .pointerHoverIcon(PointerIcon.Text)
                    .semantics {
                        contentDescription = "Natural language or symbolic formula input field"
                    },
                label = {
                    Text(
                        "Natural Language / Symbolic Formula (e.g. NewMeasure equals A minus B)",
                        fontSize = 11.sp,
                    )
                },
                singleLine = true,
                textStyle = MaterialTheme.typography.bodyMedium.copy(
                    fontFamily = FontFamily.Monospace,
                    fontSize = 13.sp,
                ),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { onSubmitFormula() }),
            )
            Button(
                onClick = onSubmitFormula,
                modifier = Modifier
                    .pointerHoverIcon(PointerIcon.Hand)
                    .semantics { contentDescription = "Evaluate and apply formula" },
                colors = ButtonDefaults.buttonColors(containerColor = NextAccentBlue),
            ) {
                Text("Evaluate")
            }
        }

        if (formulaError != null) {
            Text(
                text = "⚠ $formulaError",
                color = MaterialTheme.colorScheme.error,
                fontSize = 12.sp,
                fontFamily = FontFamily.Monospace,
            )
        }

        // Autocomplete Suggestion Chips
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                text = "Autocomplete:",
                fontSize = 11.sp,
                fontWeight = FontWeight.SemiBold,
                color = Color(0xFF475569),
            )
            autocompleteSuggestions.forEach { suggestion ->
                AssistChip(
                    onClick = { onApplySuggestion(suggestion) },
                    label = {
                        Text(
                            text = suggestion,
                            fontSize = 11.sp,
                            fontFamily = FontFamily.Monospace,
                        )
                    },
                    modifier = Modifier
                        .height(28.dp)
                        .pointerHoverIcon(PointerIcon.Hand)
                        .semantics {
                            contentDescription = "Autocomplete suggestion $suggestion"
                        },
                    colors = AssistChipDefaults.assistChipColors(
                        containerColor = Color(0xFFE2E8F0),
                    ),
                )
            }
        }

        // Active Model Formulas List
        Column(
            modifier = Modifier.fillMaxWidth(),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            formulas.forEachIndexed { idx, entry ->
                Surface(
                    modifier = Modifier
                        .fillMaxWidth()
                        .pointerHoverIcon(PointerIcon.Hand)
                        .semantics {
                            contentDescription =
                                "Formula ${idx + 1}: ${entry.expression} (${entry.naturalLanguage})"
                        }
                        .clickable { onSelectExistingFormula(entry) },
                    color = Color(0xFFF8FAFC),
                    shape = RoundedCornerShape(4.dp),
                ) {
                    Row(
                        modifier = Modifier
                            .border(0.5.dp, Color(0xFFCBD5E1), RoundedCornerShape(4.dp))
                            .padding(horizontal = 10.dp, vertical = 5.dp),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            text = "✓ ${idx + 1}. ${entry.expression}",
                            fontSize = 12.sp,
                            fontFamily = FontFamily.Monospace,
                            fontWeight = FontWeight.SemiBold,
                            color = Color(0xFF0F172A),
                        )
                        Spacer(modifier = Modifier.width(12.dp))
                        Text(
                            text = "“${entry.naturalLanguage}”",
                            fontSize = 11.sp,
                            color = Color(0xFF475569),
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
            }
        }
    }
}

/**
 * Selected Cell Semantic Coordinate Bar + inline numeric input editor for input measures.
 */
@Composable
fun SemanticCoordinateBar(
    uiState: ImprovUiState,
    onEditBufferChanged: (String) -> Unit,
    onCommitCellEdit: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier
            .fillMaxWidth()
            .background(Color(0xFF1E293B))
            .padding(horizontal = 12.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Column(modifier = Modifier.weight(1f)) {
            Text(
                text = uiState.selectedCellCoordinate,
                color = Color(0xFF93C5FD),
                fontSize = 12.sp,
                fontFamily = FontFamily.Monospace,
                fontWeight = FontWeight.Bold,
                modifier = Modifier.semantics {
                    contentDescription = "Selected cell semantic coordinate ${uiState.selectedCellCoordinate}"
                },
            )
            Text(
                text = uiState.statusMessage,
                color = Color(0xFFCBD5E1),
                fontSize = 11.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }

        Spacer(modifier = Modifier.width(8.dp))

        if (uiState.selectedCellIsDerived) {
            Box(
                modifier = Modifier
                    .background(Color(0xFF334155), RoundedCornerShape(4.dp))
                    .padding(horizontal = 8.dp, vertical = 4.dp),
            ) {
                Text(
                    text = "Derived = ${uiState.selectedCellFormatted}",
                    color = Color(0xFFBAE6FD),
                    fontSize = 11.sp,
                    fontFamily = FontFamily.Monospace,
                )
            }
        } else {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                OutlinedTextField(
                    value = uiState.selectedCellEditBuffer,
                    onValueChange = onEditBufferChanged,
                    modifier = Modifier
                        .width(110.dp)
                        .pointerHoverIcon(PointerIcon.Text)
                        .semantics { contentDescription = "Selected input cell value editor" },
                    singleLine = true,
                    textStyle = MaterialTheme.typography.bodySmall.copy(
                        color = Color.White,
                        fontFamily = FontFamily.Monospace,
                    ),
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { onCommitCellEdit() }),
                )
                OutlinedButton(
                    onClick = onCommitCellEdit,
                    contentPadding = PaddingValues(horizontal = 10.dp, vertical = 4.dp),
                    modifier = Modifier.pointerHoverIcon(PointerIcon.Hand),
                ) {
                    Text("Set", color = Color.White, fontSize = 11.sp)
                }
            }
        }
    }
}

/**
 * Page filter item selector strip for categories placed on the Page axis.
 */
@Composable
fun PageCategoryFilterStrip(
    pageCategories: List<CategoryTile>,
    onSelectPageItem: (String, String) -> Unit,
    modifier: Modifier = Modifier,
) {
    if (pageCategories.isEmpty()) return
    Row(
        modifier = modifier
            .fillMaxWidth()
            .background(Color(0xFFF1F5F9))
            .padding(horizontal = 12.dp, vertical = 4.dp)
            .horizontalScroll(rememberScrollState()),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        pageCategories.forEach { cat ->
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Text(
                    text = "Page [${cat.name}]:",
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Bold,
                    color = Color(0xFF92400E),
                )
                cat.items.forEach { item ->
                    val isSelected = item.id == cat.selectedPageItemId
                    Box(
                        modifier = Modifier
                            .pointerHoverIcon(PointerIcon.Hand)
                            .semantics {
                                contentDescription = "Select page item ${item.name} for ${cat.name}"
                            }
                            .background(
                                if (isSelected) Color(0xFFD97706) else Color(0xFFE2E8F0),
                                RoundedCornerShape(4.dp),
                            )
                            .clickable { onSelectPageItem(cat.id, item.id) }
                            .padding(horizontal = 8.dp, vertical = 3.dp),
                    ) {
                        Text(
                            text = item.name,
                            fontSize = 11.sp,
                            fontWeight = if (isSelected) FontWeight.Bold else FontWeight.Normal,
                            color = if (isSelected) Color.White else Color(0xFF1E293B),
                        )
                    }
                }
            }
        }
    }
}

/**
 * Full Analytical Studio panel providing:
 * 1. Real-time Statistical Slice Summary (Sum, Mean, Median, Min, Max, StdDev) + visual bar distribution
 * 2. One-tap What-If Scenario & Sensitivity Shocks
 * 3. Dynamic Schema Authoring (add category items, add/delete input measures, create blank workbook)
 * 4. CSV Data Import / Export powered by `improv_storage_csv`
 */
@Composable
fun AnalyticalStudioSection(
    uiState: ImprovUiState,
    viewModel: ImprovViewModel,
    modifier: Modifier = Modifier,
) {
    var targetCatForItem by remember(uiState.rowCategories, uiState.columnCategories, uiState.pageCategories) {
        val firstCat = (uiState.rowCategories + uiState.columnCategories + uiState.pageCategories).firstOrNull()?.name
        mutableStateOf(firstCat ?: "")
    }
    var newItemName by remember { mutableStateOf("") }
    var newMeasureName by remember { mutableStateOf("") }
    var newMeasureDefault by remember { mutableStateOf("0.0") }

    val stats = uiState.sliceAnalytics
    val fmt = { v: Double -> String.format(java.util.Locale.US, "%,.2f", v) }

    Column(
        modifier = modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        // 1. Statistical Slice Analytics & Distribution Chart
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .background( Color(0xFF0F172A), RoundedCornerShape(3.dp))
                .padding(horizontal = 8.dp, vertical = 5.dp),
        ) {
            Text(
                text = "SLICE ANALYTICS · ${stats.measureName.uppercase()}",
                color = Color(0xFF93C5FD),
                fontSize = 11.sp,
                fontWeight = FontWeight.Bold,
                fontFamily = FontFamily.Monospace,
            )
        }

        // Measure selector chips for analytics
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            uiState.measures.forEach { m ->
                val isSelected = m.name.equals(uiState.activeAnalyticsMeasure, ignoreCase = true)
                Box(
                    modifier = Modifier
                        .pointerHoverIcon(PointerIcon.Hand)
                        .background(
                            if (isSelected) NextAccentBlue else Color(0xFFE2E8F0),
                            RoundedCornerShape(3.dp),
                        )
                        .clickable { viewModel.selectAnalyticsMeasure(m.name) }
                        .padding(horizontal = 7.dp, vertical = 3.dp),
                ) {
                    Text(
                        text = m.name,
                        fontSize = 10.sp,
                        fontWeight = FontWeight.SemiBold,
                        color = if (isSelected) Color.White else Color(0xFF1E293B),
                    )
                }
            }
        }

        // Key Statistical KPIs (2x3 grid)
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .background(Color.White, RoundedCornerShape(4.dp))
                .border(0.5.dp, Color(0xFFCBD5E1), RoundedCornerShape(4.dp))
                .padding(8.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text("Sum: ${fmt(stats.sum)}", fontSize = 11.sp, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Bold, color = Color(0xFF0F172A))
                Text("Mean: ${fmt(stats.mean)}", fontSize = 11.sp, fontFamily = FontFamily.Monospace, color = Color(0xFF1E3A8A))
            }
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text("Median: ${fmt(stats.median)}", fontSize = 11.sp, fontFamily = FontFamily.Monospace, color = Color(0xFF334155))
                Text("StdDev: ${fmt(stats.stdDev)}", fontSize = 11.sp, fontFamily = FontFamily.Monospace, color = Color(0xFF475569))
            }
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text("Min: ${fmt(stats.min)}", fontSize = 11.sp, fontFamily = FontFamily.Monospace, color = Color(0xFF047857))
                Text("Max: ${fmt(stats.max)} (n=${stats.cellCount})", fontSize = 11.sp, fontFamily = FontFamily.Monospace, color = Color(0xFFB45309))
            }

            // Inline Series Distribution Bars
            if (stats.seriesValues.isNotEmpty()) {
                val maxAbs = stats.seriesValues.maxOf { kotlin.math.abs(it) }.coerceAtLeast(1.0)
                Spacer(modifier = Modifier.height(2.dp))
                stats.seriesLabels.zip(stats.seriesValues).take(6).forEach { (label, value) ->
                    val fraction = (kotlin.math.abs(value) / maxAbs).toFloat().coerceIn(0.05f, 1f)
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        Text(
                            text = label,
                            fontSize = 10.sp,
                            fontFamily = FontFamily.Monospace,
                            color = Color(0xFF334155),
                            modifier = Modifier.width(56.dp),
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Box(
                            modifier = Modifier
                                .weight(1f)
                                .height(10.dp)
                                .background(Color(0xFFF1F5F9), RoundedCornerShape(2.dp)),
                        ) {
                            Box(
                                modifier = Modifier
                                    .fillMaxWidth(fraction)
                                    .height(10.dp)
                                    .background(
                                        if (value >= 0.0) Color(0xFF2563EB) else Color(0xFFDC2626),
                                        RoundedCornerShape(2.dp),
                                    ),
                            )
                        }
                        Text(
                            text = fmt(value),
                            fontSize = 9.sp,
                            fontFamily = FontFamily.Monospace,
                            color = Color(0xFF0F172A),
                        )
                    }
                }
            }
        }

        // 2. What-If Scenario & Sensitivity Simulator.  The buttons derive from
        // the model's own input measures, so they work for any workbook rather
        // than only the sample's Quantity/Price/UnitCost.
        val inputMeasures = uiState.measures.filter { !it.isDerived }
        if (inputMeasures.isNotEmpty()) {
            Text(
                text = "What-If Scenario Shocks (+10%)",
                fontSize = 12.sp,
                fontWeight = FontWeight.Bold,
                color = Color(0xFF0F172A),
            )
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                inputMeasures.take(3).forEach { m ->
                    OutlinedButton(
                        onClick = { viewModel.applyWhatIfMultiplier(m.name, 1.10, "+10% ${m.name}") },
                        contentPadding = PaddingValues(horizontal = 6.dp, vertical = 2.dp),
                        modifier = Modifier.weight(1f).pointerHoverIcon(PointerIcon.Hand),
                    ) {
                        Text("+10% ${m.name}", fontSize = 10.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
        }

        // 3. Schema Authoring: Add Item to Category & Add Input Measure
        Text(
            text = "Expand Category Items & Measures",
            fontSize = 12.sp,
            fontWeight = FontWeight.Bold,
            color = Color(0xFF0F172A),
        )
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            OutlinedTextField(
                value = targetCatForItem,
                onValueChange = { targetCatForItem = it },
                label = { Text("Category", fontSize = 10.sp) },
                singleLine = true,
                modifier = Modifier.weight(1f).pointerHoverIcon(PointerIcon.Text),
            )
            OutlinedTextField(
                value = newItemName,
                onValueChange = { newItemName = it },
                label = { Text("New Item", fontSize = 10.sp) },
                singleLine = true,
                modifier = Modifier.weight(1f).pointerHoverIcon(PointerIcon.Text),
            )
            Button(
                onClick = { viewModel.addCategoryItem(targetCatForItem, newItemName) },
                contentPadding = PaddingValues(horizontal = 8.dp, vertical = 4.dp),
                modifier = Modifier.pointerHoverIcon(PointerIcon.Hand),
                colors = ButtonDefaults.buttonColors(containerColor = NextDarkChrome),
            ) {
                Text("+Item", fontSize = 10.sp)
            }
        }

        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            OutlinedTextField(
                value = newMeasureName,
                onValueChange = { newMeasureName = it },
                label = { Text("Input Measure", fontSize = 10.sp) },
                singleLine = true,
                modifier = Modifier.weight(1f).pointerHoverIcon(PointerIcon.Text),
            )
            OutlinedTextField(
                value = newMeasureDefault,
                onValueChange = { newMeasureDefault = it },
                label = { Text("Default", fontSize = 10.sp) },
                singleLine = true,
                modifier = Modifier. width(76.dp).pointerHoverIcon(PointerIcon.Text),
            )
            Button(
                onClick = { viewModel.addInputMeasure(newMeasureName, newMeasureDefault) },
                contentPadding = PaddingValues(horizontal = 8.dp, vertical = 4.dp),
                modifier = Modifier.pointerHoverIcon(PointerIcon.Hand),
                colors = ButtonDefaults.buttonColors(containerColor = NextAccentBlue),
            ) {
                Text("+Measure", fontSize = 10.sp)
            }
        }

        // 4. CSV Data Import / Export Workbench
        Text(
            text = "CSV Import / Export (improv_storage_csv)",
            fontSize = 12.sp,
            fontWeight = FontWeight.Bold,
            color = Color(0xFF0F172A),
        )
        OutlinedTextField(
            value = uiState.csvImportExportBuffer,
            onValueChange = viewModel::onCsvBufferChanged,
            label = { Text("Headered CSV (Dim1,Dim2,...,Measure)", fontSize = 10.sp) },
            maxLines = 4,
            textStyle = MaterialTheme.typography.bodySmall.copy(
                fontFamily = FontFamily.Monospace,
                fontSize = 10.sp,
            ),
            modifier = Modifier
                .fillMaxWidth()
                .pointerHoverIcon(PointerIcon.Text),
        )
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            OutlinedButton(
                onClick = { viewModel.exportMeasureToCsvBuffer(uiState.activeAnalyticsMeasure) },
                contentPadding = PaddingValues(horizontal = 8.dp, vertical = 4.dp),
                modifier = Modifier.weight(1f).pointerHoverIcon(PointerIcon.Hand),
            ) {
                Text("Export '${uiState.activeAnalyticsMeasure}' CSV", fontSize = 10.sp)
            }
            Button(
                onClick = { viewModel.importCsvFromBuffer() },
                contentPadding = PaddingValues(horizontal = 8.dp, vertical = 4.dp),
                modifier = Modifier.weight(1f).pointerHoverIcon(PointerIcon.Hand),
                colors = ButtonDefaults.buttonColors(containerColor = Color(0xFF047857)),
            ) {
                Text("Import CSV to Cube", fontSize = 10.sp)
            }
        }
    }
}
