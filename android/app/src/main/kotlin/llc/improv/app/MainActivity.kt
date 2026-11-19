package llc.improv.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.ui.Modifier
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import llc.improv.app.ui.ImprovAdaptiveApp
import llc.improv.app.ui.ImprovViewModel
import llc.improv.app.ui.theme.ImprovTheme
import llc.improv.app.workbook.ModelStore
import java.io.File

class MainActivity : ComponentActivity() {

    // The ViewModel owns model lifecycle; the Activity only supplies the
    // workbook directory.  No single engine is pinned for the app's life.
    private val improvViewModel: ImprovViewModel by viewModels {
        val store = ModelStore(File(filesDir, "workbooks"))
        object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T {
                return ImprovViewModel(modelStore = store) as T
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            ImprovTheme {
                Surface(
                    modifier = Modifier
                        .fillMaxSize()
                        .safeDrawingPadding(),
                    color = MaterialTheme.colorScheme.background,
                ) {
                    ImprovAdaptiveApp(viewModel = improvViewModel)
                }
            }
        }
    }
}
