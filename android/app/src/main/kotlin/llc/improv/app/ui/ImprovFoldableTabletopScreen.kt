package llc.improv.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * Foldable Tabletop ("Laptop" / Half-Opened Hinge) Posture Screen:
 * - **Top Display Half (Angled toward viewer)**: 3D Multidimensional Spreadsheet Grid
 * - **Hinge Divider**: Visual physical hinge boundary
 * - **Bottom Display Half (Flat on table)**: Tactile Category Tile Pivot Deck, Formula Pad, and Analytical Studio
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ImprovFoldableTabletopScreen(
    uiState: ImprovUiState,
    viewModel: ImprovViewModel,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier = modifier
            .fillMaxSize()
            .background(NextPlatinumBg)
            .semantics { contentDescription = "Improv Foldable Tabletop Split-Hinge Screen" },
    ) {
        // ==================== TOP HALF: VIEWER-ANGLED DISPLAY ====================
        Column(
            modifier = Modifier
                .weight(0.52f)
                .fillMaxWidth(),
        ) {
            // Tabletop Header Bar
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .background(NextDarkChrome)
                    .padding(horizontal = 12.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    modifier = Modifier.weight(1f),
                ) {
                    Box(
                        modifier = Modifier
                            .background(Color(0xFF4F46E5), RoundedCornerShape(3.dp))
                            .padding(horizontal = 7.dp, vertical = 2.dp),
                    ) {
                        Text(
                            text = "FOLDABLE TABLETOP",
                            color = Color.White,
                            fontSize = 10.sp,
                            fontWeight = FontWeight.ExtraBold,
                            fontFamily = FontFamily.Monospace,
                        )
                    }
                    Text(
                        text = uiState.activeViewName,
                        color = Color(0xFFC4B5FD),
                        fontSize = 11.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }

            PageCategoryFilterStrip(
                pageCategories = uiState.pageCategories,
                onSelectPageItem = viewModel::selectPageItem,
            )

            SemanticCoordinateBar(
                uiState = uiState,
                onEditBufferChanged = viewModel::onSelectedCellEditBufferChanged,
                onCommitCellEdit = viewModel::commitSelectedCellEdit,
            )

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

        // ==================== PHYSICAL FOLDABLE HINGE SEPARATOR ====================
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .background(Color(0xFF0F172A))
                .border(1.dp, Color(0xFF475569))
                .padding(vertical = 3.dp),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                text = "═══ FOLDABLE HINGE · TOP: 3D CUBE VIEWPORT  |  BOTTOM: TACTILE AI & PIVOT CONTROL DECK ═══",
                color = Color(0xFFFDE68A),
                fontSize = 9.sp,
                fontWeight = FontWeight.Bold,
                fontFamily = FontFamily.Monospace,
            )
        }

        // ==================== BOTTOM HALF: FLAT TACTILE CONTROL DECK ====================
        Row(
            modifier = Modifier
                .weight(0.48f)
                .fillMaxWidth()
                .background(NextPanelSurface)
                .padding(8.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            // Left Control Deck: Tactile Category Tile Pivot Pad + Stylus Formula Editor
            Surface(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxHeight()
                    .border(1.dp, NextBorderDark, RoundedCornerShape(4.dp)),
                color = Color.White,
                shape = RoundedCornerShape(4.dp),
            ) {
                Column(
                    modifier = Modifier
                        .fillMaxSize()
                        .padding(8.dp)
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Text(
                        text = "TACTILE CATEGORY TILE DECK & FORMULA PAD",
                        fontSize = 11.sp,
                        fontWeight = FontWeight.Bold,
                        fontFamily = FontFamily.Monospace,
                        color = Color(0xFF0F172A),
                    )
                    FlowRow(
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                        verticalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        (uiState.rowCategories + uiState.columnCategories + uiState.pageCategories).forEach { tile ->
                            CategoryTileChip(
                                tile = tile,
                                onPivotTo = { axis -> viewModel.pivotCategory(tile.id, axis, 0) },
                                onCycleAxis = { viewModel.cycleCategoryAxis(tile) },
                            )
                        }
                    }

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

            // Right Control Deck: Analytical Studio, What-If Shocks & CSV I/O
            Surface(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxHeight()
                    .border(1.dp, NextBorderDark, RoundedCornerShape(4.dp)),
                color = NextPanelSurface,
                shape = RoundedCornerShape(4.dp),
            ) {
                Column(
                    modifier = Modifier
                        .fillMaxSize()
                        .padding(8.dp)
                        .verticalScroll(rememberScrollState()),
                ) {
                    AnalyticalStudioSection(
                        uiState = uiState,
                        viewModel = viewModel,
                    )
                }
            }
        }
    }
}
