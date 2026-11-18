package llc.improv.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.PointerIcon
import androidx.compose.ui.input.pointer.pointerHoverIcon
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import uniffi.improv_android_ffi.AxisOrientation

private enum class PhoneBottomSheetKind {
    NONE,
    CATEGORIES,
    FORMULAS,
    ANALYTICS,
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun ImprovPhoneScreen(
    uiState: ImprovUiState,
    viewModel: ImprovViewModel,
    modifier: Modifier = Modifier,
    onToggleLayoutOverride: (() -> Unit)? = null,
) {
    var activeSheet by remember { mutableStateOf(PhoneBottomSheetKind.NONE) }
    var newCategoryName by remember { mutableStateOf("") }
    var newCategoryItems by remember { mutableStateOf("North, South") }

    Scaffold(
        modifier = modifier
            .fillMaxSize()
            .semantics { contentDescription = "Improv Phone Compact Screen" },
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(
                            text = "Improv 3D",
                            fontSize = 16.sp,
                            fontWeight = FontWeight.Bold,
                            color = Color.White,
                        )
                        Text(
                            text = uiState.activeViewName,
                            fontSize = 11.sp,
                            color = Color(0xFF93C5FD),
                            fontFamily = FontFamily.Monospace,
                        )
                    }
                },
                actions = {
                    TextButton(
                        onClick = { activeSheet = PhoneBottomSheetKind.CATEGORIES },
                        modifier = Modifier
                            .pointerHoverIcon(PointerIcon.Hand)
                            .semantics { contentDescription = "Open Category Tiles Pivot Bottom Sheet" },
                    ) {
                        Text("Pivot", color = Color(0xFFFDE68A), fontWeight = FontWeight.SemiBold)
                    }
                    TextButton(
                        onClick = { activeSheet = PhoneBottomSheetKind.FORMULAS },
                        modifier = Modifier
                            .pointerHoverIcon(PointerIcon.Hand)
                            .semantics { contentDescription = "Open Natural Language Formulas Bottom Sheet" },
                    ) {
                        Text("Formulas", color = Color(0xFF93C5FD), fontWeight = FontWeight.SemiBold)
                    }
                    TextButton(
                        onClick = { activeSheet = PhoneBottomSheetKind.ANALYTICS },
                        modifier = Modifier
                            .pointerHoverIcon(PointerIcon.Hand)
                            .semantics { contentDescription = "Open Analytics and What-If Studio Bottom Sheet" },
                    ) {
                        Text("Analytics", color = Color(0xFF86EFAC), fontWeight = FontWeight.SemiBold)
                    }
                    if (onToggleLayoutOverride != null) {
                        TextButton(
                            onClick = onToggleLayoutOverride,
                            modifier = Modifier.pointerHoverIcon(PointerIcon.Hand),
                        ) {
                            Text("Desktop", color = Color(0xFFCBD5E1), fontSize = 11.sp)
                        }
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = NextDarkChrome,
                ),
            )
        },
        bottomBar = {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .background(NextDarkChrome)
                    .padding(horizontal = 12.dp, vertical = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Button(
                    onClick = { activeSheet = PhoneBottomSheetKind.CATEGORIES },
                    modifier = Modifier
                        .weight(1f)
                        .pointerHoverIcon(PointerIcon.Hand)
                        .semantics { contentDescription = "Pivot Categories Bottom Sheet Button" },
                ) {
                    Text("Pivot Categories", fontSize = 12.sp)
                }
                Button(
                    onClick = { activeSheet = PhoneBottomSheetKind.FORMULAS },
                    modifier = Modifier
                        .weight(1f)
                        .pointerHoverIcon(PointerIcon.Hand)
                        .semantics { contentDescription = "Natural Language Formulas Bottom Sheet Button" },
                ) {
                    Text("NL Formulas (${uiState.formulas.size})", fontSize = 12.sp)
                }
                OutlinedButton(
                    onClick = { viewModel.resetSampleModel() },
                    modifier = Modifier.pointerHoverIcon(PointerIcon.Hand),
                ) {
                    Text("Reset", color = Color.White, fontSize = 11.sp)
                }
            }
        },
    ) { innerPadding ->
        Column(
            modifier = Modifier
                .padding(innerPadding)
                .fillMaxSize()
                .background(NextPlatinumBg),
        ) {
            // Compact Category Tile Strip
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .background(NextMidChrome)
                    .padding(horizontal = 10.dp, vertical = 6.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                (uiState.rowCategories + uiState.columnCategories + uiState.pageCategories).forEach { tile ->
                    CategoryTileChip(
                        tile = tile,
                        onPivotTo = { axis -> viewModel.pivotCategory(tile.id, axis, 0) },
                        onCycleAxis = { viewModel.cycleCategoryAxis(tile) },
                    )
                }
            }

            // Page Filter Selector Strip
            PageCategoryFilterStrip(
                pageCategories = uiState.pageCategories,
                onSelectPageItem = viewModel::selectPageItem,
            )

            // Semantic Coordinate & Cell Value Bar
            SemanticCoordinateBar(
                uiState = uiState,
                onEditBufferChanged = viewModel::onSelectedCellEditBufferChanged,
                onCommitCellEdit = viewModel::commitSelectedCellEdit,
            )

            // Scrollable 2D Slice of the Multidimensional Grid
            SpreadsheetGridViewport(
                uiState = uiState,
                onSelectCell = viewModel::selectCell,
                onMoveFocus = viewModel::moveCellFocus,
                onTabFocus = viewModel::moveTabFocus,
                onScrollChanged = viewModel::updateScrollIndices,
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth(),
            )
        }

        // Collapsible Bottom Sheet: Category Tiles Pivot & Dimension Manager
        if (activeSheet == PhoneBottomSheetKind.CATEGORIES) {
            ModalBottomSheet(
                onDismissRequest = { activeSheet = PhoneBottomSheetKind.NONE },
                sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            ) {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 8.dp),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Text(
                        text = "Pivot Category Tiles (Row / Column / Page)",
                        style = MaterialTheme.typography.titleMedium,
                        fontWeight = FontWeight.Bold,
                    )
                    Text(
                        text = "Tap a tile or its R / C / P badge to rearrange the 3D cube axes.",
                        fontSize = 12.sp,
                        color = Color(0xFF475569),
                    )
                    FlowRow(
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        (uiState.rowCategories + uiState.columnCategories + uiState.pageCategories).forEach { tile ->
                            CategoryTileChip(
                                tile = tile,
                                onPivotTo = { axis -> viewModel.pivotCategory(tile.id, axis, 0) },
                                onCycleAxis = { viewModel.cycleCategoryAxis(tile) },
                            )
                        }
                    }

                    HorizontalDivider()
                    Text(
                        text = "Add New Category Dimension",
                        style = MaterialTheme.typography.titleSmall,
                        fontWeight = FontWeight.SemiBold,
                    )
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        OutlinedTextField(
                            value = newCategoryName,
                            onValueChange = { newCategoryName = it },
                            label = { Text("Category Name (e.g. Channel)") },
                            singleLine = true,
                            modifier = Modifier
                                .weight(1f)
                                .pointerHoverIcon(PointerIcon.Text),
                        )
                        OutlinedTextField(
                            value = newCategoryItems,
                            onValueChange = { newCategoryItems = it },
                            label = { Text("Items CSV") },
                            singleLine = true,
                            modifier = Modifier
                                .weight(1f)
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
                            modifier = Modifier.pointerHoverIcon(PointerIcon.Hand),
                        ) {
                            Text("Add")
                        }
                    }
                    Spacer(modifier = Modifier.height(16.dp))
                }
            }
        }

        // Collapsible Bottom Sheet: Natural Language Formulas + Autocomplete Chips
        if (activeSheet == PhoneBottomSheetKind.FORMULAS) {
            ModalBottomSheet(
                onDismissRequest = { activeSheet = PhoneBottomSheetKind.NONE },
                sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            ) {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 8.dp),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Text(
                        text = "Natural Language & Symbolic Formulas",
                        style = MaterialTheme.typography.titleMedium,
                        fontWeight = FontWeight.Bold,
                    )
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
                    Spacer(modifier = Modifier.height(16.dp))
                }
            }
        }

        // Collapsible Bottom Sheet: Analytics, What-If Shocks, Schema Authoring & CSV I/O
        if (activeSheet == PhoneBottomSheetKind.ANALYTICS) {
            ModalBottomSheet(
                onDismissRequest = { activeSheet = PhoneBottomSheetKind.NONE },
                sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            ) {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 8.dp)
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Text(
                        text = "Analytics, What-If Studio & CSV Data I/O",
                        style = MaterialTheme.typography.titleMedium,
                        fontWeight = FontWeight.Bold,
                    )
                    AnalyticalStudioSection(
                        uiState = uiState,
                        viewModel = viewModel,
                    )
                    Spacer(modifier = Modifier.height(16.dp))
                }
            }
        }
    }
}
