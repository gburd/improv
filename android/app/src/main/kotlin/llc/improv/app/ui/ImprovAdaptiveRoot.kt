package llc.improv.app.ui

import androidx.compose.material3.adaptive.currentWindowAdaptiveInfo
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.window.core.layout.WindowSizeClass
import androidx.window.core.layout.WindowWidthSizeClass

/**
 * Adaptive root.  First branch: workbook open or not.  With no open workbook
 * the app shows the [WorkbookLibraryScreen] (so a first launch never drops the
 * user into a model).  With a workbook open, it routes by `WindowSizeClass`:
 * - `COMPACT` width (phone portrait) -> [ImprovPhoneScreen], touch-first, the
 *   grid as the hero with pivot/formulas/analytics in bottom sheets.
 * - `MEDIUM`/`EXPANDED` width (tablet, Chromebook/Googlebook, desktop mode) ->
 *   [ImprovDesktopScreen], a multi-pane left-rail + grid + right-inspector
 *   layout with draggable tiles, keyboard and hover.
 * - tabletop posture -> [ImprovFoldableTabletopScreen].
 *
 * Both model layouts and the library share one [ImprovViewModel] and model.
 */
@Composable
fun ImprovAdaptiveApp(
    viewModel: ImprovViewModel,
    modifier: Modifier = Modifier,
    windowSizeClassOverride: WindowSizeClass? = null,
) {
    val adaptiveInfo = currentWindowAdaptiveInfo()
    val windowSizeClass = windowSizeClassOverride ?: adaptiveInfo.windowSizeClass
    val isNaturalCompact = windowSizeClass.windowWidthSizeClass == WindowWidthSizeClass.COMPACT

    var manualForceCompact by rememberSaveable { mutableStateOf<Boolean?>(null) }
    val showCompact = manualForceCompact ?: isNaturalCompact

    val uiState by viewModel.uiState.collectAsStateWithLifecycle()
    val libraryState by viewModel.libraryState.collectAsStateWithLifecycle()
    val isTabletop = adaptiveInfo.windowPosture.isTabletop

    if (uiState.activeModelId == null) {
        WorkbookLibraryScreen(
            libraryState = libraryState,
            viewModel = viewModel,
            compact = showCompact && !isTabletop,
            modifier = modifier,
        )
        return
    }

    if (isTabletop) {
        ImprovFoldableTabletopScreen(
            uiState = uiState,
            viewModel = viewModel,
            modifier = modifier,
        )
    } else if (showCompact) {
        ImprovPhoneScreen(
            uiState = uiState,
            viewModel = viewModel,
            onToggleLayoutOverride = { manualForceCompact = false },
            modifier = modifier,
        )
    } else {
        ImprovDesktopScreen(
            uiState = uiState,
            viewModel = viewModel,
            onToggleLayoutOverride = { manualForceCompact = true },
            modifier = modifier,
        )
    }
}
