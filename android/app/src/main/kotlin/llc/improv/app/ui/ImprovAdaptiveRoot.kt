package llc.improv.app.ui

import androidx.compose.material3.adaptive.currentWindowAdaptiveInfo
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.window.core.layout.WindowSizeClass
import androidx.window.core.layout.WindowWidthSizeClass

/**
 * Adaptive root composable using `androidx.compose.material3.adaptive` and `WindowSizeClass`:
 * - When `WindowWidthSizeClass.COMPACT` is active (Pixel Phone portrait), renders [ImprovPhoneScreen].
 * - When `WindowWidthSizeClass.MEDIUM` or `WindowWidthSizeClass.EXPANDED` is active
 *   (HP Googlebook 14 or Pixel in Desktop Mode), renders [ImprovDesktopScreen].
 */
@Composable
fun ImprovAdaptiveApp(
    modifier: Modifier = Modifier,
    viewModel: ImprovViewModel = viewModel { ImprovViewModel() },
    windowSizeClassOverride: WindowSizeClass? = null,
) {
    val adaptiveInfo = currentWindowAdaptiveInfo()
    val windowSizeClass = windowSizeClassOverride ?: adaptiveInfo.windowSizeClass
    val isNaturalCompact = windowSizeClass.windowWidthSizeClass == WindowWidthSizeClass.COMPACT

    var manualForceCompact by rememberSaveable { mutableStateOf<Boolean?>(null) }
    val showCompact = manualForceCompact ?: isNaturalCompact

    val uiState by viewModel.uiState.collectAsStateWithLifecycle()
    val isTabletop = adaptiveInfo.windowPosture.isTabletop

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
