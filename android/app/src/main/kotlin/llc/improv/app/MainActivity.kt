package llc.improv.app

import android.content.Intent
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
import uniffi.improv_android_ffi.ImprovMobileEngine
import java.io.File

class MainActivity : ComponentActivity() {

    private val improvViewModel: ImprovViewModel by viewModels {
        val modelId = resolveModelIdFromIntent(intent)
        val dbPath = File(filesDir, "$modelId.db").absolutePath
        object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T {
                return ImprovViewModel(
                    engine = ImprovMobileEngine.openOrCreate(dbPath),
                    initialModelId = modelId,
                ) as T
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

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
    }

    private fun resolveModelIdFromIntent(intent: Intent?): String {
        val fromIntent = intent?.extras?.getString(ImprovViewModel.KEY_ACTIVE_MODEL_ID)
        if (!fromIntent.isNullOrBlank()) return fromIntent
        return ImprovViewModel.DEFAULT_MODEL_ID
    }
}
