package llc.improv.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
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
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import llc.improv.app.workbook.WorkbookEntry

private enum class LibraryDialog { NONE, CREATE, RENAME, DELETE }

/**
 * The workbook library (home) screen.  Lists the workbooks on disk newest
 * first, and lets the user create (blank or example), open, rename, and delete
 * them.  Shared by both form factors: [compact] (phone) renders a single-column
 * list; the wide layout renders a multi-column card grid and wider chrome.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun WorkbookLibraryScreen(
    libraryState: LibraryUiState,
    viewModel: ImprovViewModel,
    compact: Boolean,
    modifier: Modifier = Modifier,
) {
    var dialog by remember { mutableStateOf(LibraryDialog.NONE) }
    var pendingEntry by remember { mutableStateOf<WorkbookEntry?>(null) }
    var nameField by remember { mutableStateOf("") }
    var createAsSample by remember { mutableStateOf(false) }

    Scaffold(
        modifier = modifier
            .fillMaxSize()
            .semantics { contentDescription = "Workbook Library Screen" },
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(
                            text = "Improv Workbooks",
                            fontSize = if (compact) 16.sp else 18.sp,
                            fontWeight = FontWeight.Bold,
                            color = Color.White,
                        )
                        Text(
                            text = "${libraryState.workbooks.size} workbook(s)",
                            fontSize = 11.sp,
                            color = Color(0xFF93C5FD),
                        )
                    }
                },
                actions = {
                    Button(
                        onClick = {
                            nameField = ""
                            createAsSample = false
                            dialog = LibraryDialog.CREATE
                        },
                        modifier = Modifier
                            .padding(end = 8.dp)
                            .pointerHoverIcon(PointerIcon.Hand)
                            .semantics { contentDescription = "Create a new workbook" },
                    ) {
                        Text("New")
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = NextDarkChrome),
            )
        },
    ) { innerPadding ->
        Column(
            modifier = Modifier
                .padding(innerPadding)
                .fillMaxSize()
                .background(NextPlatinumBg)
                .padding(12.dp),
        ) {
            libraryState.errorMessage?.let { err ->
                Text(
                    text = "⚠ $err",
                    color = MaterialTheme.colorScheme.error,
                    fontSize = 12.sp,
                    modifier = Modifier.padding(bottom = 8.dp),
                )
            }

            if (libraryState.workbooks.isEmpty()) {
                EmptyLibraryHint(
                    onCreateBlank = {
                        nameField = ""
                        createAsSample = false
                        dialog = LibraryDialog.CREATE
                    },
                    onCreateSample = {
                        nameField = ""
                        createAsSample = true
                        dialog = LibraryDialog.CREATE
                    },
                )
            } else if (compact) {
                LazyColumn(
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                    modifier = Modifier.fillMaxSize(),
                ) {
                    items(libraryState.workbooks, key = { it.id }) { entry ->
                        WorkbookCard(
                            entry = entry,
                            onOpen = { viewModel.openModel(entry.id) },
                            onRename = {
                                pendingEntry = entry
                                nameField = entry.displayName
                                dialog = LibraryDialog.RENAME
                            },
                            onDelete = {
                                pendingEntry = entry
                                dialog = LibraryDialog.DELETE
                            },
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                }
            } else {
                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    libraryState.workbooks.forEach { entry ->
                        WorkbookCard(
                            entry = entry,
                            onOpen = { viewModel.openModel(entry.id) },
                            onRename = {
                                pendingEntry = entry
                                nameField = entry.displayName
                                dialog = LibraryDialog.RENAME
                            },
                            onDelete = {
                                pendingEntry = entry
                                dialog = LibraryDialog.DELETE
                            },
                            modifier = Modifier.width(300.dp),
                        )
                    }
                }
            }
        }
    }

    when (dialog) {
        LibraryDialog.CREATE -> CreateWorkbookDialog(
            name = nameField,
            asSample = createAsSample,
            onNameChange = { nameField = it },
            onAsSampleChange = { createAsSample = it },
            onConfirm = {
                if (createAsSample) viewModel.createSampleWorkbook(nameField)
                else viewModel.createBlankWorkbook(nameField)
                dialog = LibraryDialog.NONE
            },
            onDismiss = { dialog = LibraryDialog.NONE },
        )
        LibraryDialog.RENAME -> AlertDialog(
            onDismissRequest = { dialog = LibraryDialog.NONE },
            title = { Text("Rename workbook") },
            text = {
                OutlinedTextField(
                    value = nameField,
                    onValueChange = { nameField = it },
                    label = { Text("Name") },
                    singleLine = true,
                    modifier = Modifier.pointerHoverIcon(PointerIcon.Text),
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    pendingEntry?.let { viewModel.renameWorkbook(it.id, nameField) }
                    dialog = LibraryDialog.NONE
                }) { Text("Rename") }
            },
            dismissButton = {
                TextButton(onClick = { dialog = LibraryDialog.NONE }) { Text("Cancel") }
            },
        )
        LibraryDialog.DELETE -> AlertDialog(
            onDismissRequest = { dialog = LibraryDialog.NONE },
            title = { Text("Delete workbook?") },
            text = {
                Text("This permanently deletes '${pendingEntry?.displayName}' and its data.")
            },
            confirmButton = {
                TextButton(onClick = {
                    pendingEntry?.let { viewModel.deleteWorkbook(it.id) }
                    dialog = LibraryDialog.NONE
                }) { Text("Delete", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = {
                TextButton(onClick = { dialog = LibraryDialog.NONE }) { Text("Cancel") }
            },
        )
        LibraryDialog.NONE -> Unit
    }
}

@Composable
private fun EmptyLibraryHint(
    onCreateBlank: () -> Unit,
    onCreateSample: () -> Unit,
) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(
            text = "No workbooks yet",
            style = MaterialTheme.typography.titleLarge,
            fontWeight = FontWeight.Bold,
            color = Color(0xFF0F172A),
        )
        Text(
            text = "Create a blank multidimensional workbook, or start from the example model.",
            fontSize = 13.sp,
            color = Color(0xFF475569),
        )
        Button(
            onClick = onCreateBlank,
            modifier = Modifier.pointerHoverIcon(PointerIcon.Hand),
        ) { Text("New blank workbook") }
        OutlinedButton(
            onClick = onCreateSample,
            modifier = Modifier.pointerHoverIcon(PointerIcon.Hand),
        ) { Text("Create example model") }
    }
}

@Composable
private fun WorkbookCard(
    entry: WorkbookEntry,
    onOpen: () -> Unit,
    onRename: () -> Unit,
    onDelete: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Box(
        modifier = modifier
            .background(Color.White, RoundedCornerShape(6.dp))
            .border(1.dp, NextBorderDark, RoundedCornerShape(6.dp))
            .clickable { onOpen() }
            .pointerHoverIcon(PointerIcon.Hand)
            .semantics { contentDescription = "Open workbook ${entry.displayName}" }
            .padding(14.dp),
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(
                text = entry.displayName,
                fontSize = 16.sp,
                fontWeight = FontWeight.SemiBold,
                color = Color(0xFF0F172A),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = entry.summary,
                fontSize = 12.sp,
                fontFamily = FontFamily.Monospace,
                color = Color(0xFF64748B),
            )
            Spacer(modifier = Modifier.height(2.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(
                    onClick = onRename,
                    modifier = Modifier.pointerHoverIcon(PointerIcon.Hand),
                ) { Text("Rename", fontSize = 12.sp) }
                OutlinedButton(
                    onClick = onDelete,
                    colors = ButtonDefaults.outlinedButtonColors(contentColor = MaterialTheme.colorScheme.error),
                    modifier = Modifier.pointerHoverIcon(PointerIcon.Hand),
                ) { Text("Delete", fontSize = 12.sp) }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun CreateWorkbookDialog(
    name: String,
    asSample: Boolean,
    onNameChange: (String) -> Unit,
    onAsSampleChange: (Boolean) -> Unit,
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("New workbook") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                OutlinedTextField(
                    value = name,
                    onValueChange = onNameChange,
                    label = { Text("Name") },
                    singleLine = true,
                    modifier = Modifier
                        .fillMaxWidth()
                        .pointerHoverIcon(PointerIcon.Text),
                )
                Text("Start from:", fontSize = 12.sp, color = Color(0xFF475569))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    TemplateChoice("Blank starter", !asSample) { onAsSampleChange(false) }
                    TemplateChoice("Example model", asSample) { onAsSampleChange(true) }
                }
            }
        },
        confirmButton = {
            TextButton(
                onClick = onConfirm,
                modifier = Modifier.semantics { contentDescription = "Confirm create workbook" },
            ) { Text("Create") }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text("Cancel") }
        },
    )
}

@Composable
private fun TemplateChoice(label: String, selected: Boolean, onClick: () -> Unit) {
    Box(
        modifier = Modifier
            .background(
                if (selected) NextAccentBlue else Color(0xFFE2E8F0),
                RoundedCornerShape(4.dp),
            )
            .clickable { onClick() }
            .pointerHoverIcon(PointerIcon.Hand)
            .padding(horizontal = 10.dp, vertical = 6.dp),
    ) {
        Text(
            text = label,
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
            color = if (selected) Color.White else Color(0xFF1E293B),
        )
    }
}
