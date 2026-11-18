package llc.improv.app

import com.sun.jna.Native
import com.sun.jna.Pointer
import uniffi.improv_android_ffi.ImprovMobileEngine
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Populates a direct [ByteBuffer] (`ByteBuffer.allocateDirect(...)`) in-place with the
 * visible cell viewport `[startRow..startRow+rowCount, startCol..startCol+colCount]`.
 *
 * Binary layout (Little-Endian):
 * - Header (16 bytes): `[rows: u32, cols: u32, totalRows: u32, totalCols: u32]`
 * - Cell records (`rows * cols * 24` bytes):
 *   - `+0..8`:   `f64` numeric value (`Double.NaN` if blank/error)
 *   - `+8..12`:  `u32` measure ID
 *   - `+12..16`: `u32` primary row item ID
 *   - `+16..20`: `u32` primary column item ID
 *   - `+20`:     `u8`  cell tag (`0 = Blank`, `1 = InputNumber`, `2 = DerivedNumber`, `3 = NonNumeric`, `4 = Error`)
 *   - `+21`:     `u8`  `isDerived` (`0` or `1`)
 *   - `+22..24`: `u16` reserved (`0`)
 */
fun ImprovMobileEngine.populateDirectByteBuffer(
    buffer: ByteBuffer,
    startRow: UInt = 0u,
    rowCount: UInt = 64u,
    startCol: UInt = 0u,
    colCount: UInt = 16u,
): Int {
    require(buffer.isDirect) { "buffer must be allocated with ByteBuffer.allocateDirect()" }
    buffer.order(ByteOrder.LITTLE_ENDIAN)
    val ptr = Native.getDirectBufferPointer(buffer)
    val addr = Pointer.nativeValue(ptr).toULong()
    val written = populateViewportBuffer(
        bufferPtr = addr,
        bufferCapacity = buffer.capacity().toUInt(),
        startRow = startRow,
        rowCount = rowCount,
        startCol = startCol,
        colCount = colCount,
    ).toInt()
    buffer.position(0)
    buffer.limit(written.coerceAtMost(buffer.capacity()))
    return written
}

/**
 * Zero-allocation reader over a direct [ByteBuffer] populated by [populateDirectByteBuffer].
 */
class DirectViewportReader(private val buffer: ByteBuffer) {
    init {
        buffer.order(ByteOrder.LITTLE_ENDIAN)
    }

    val rows: Int get() = if (buffer.limit() >= 16) buffer.getInt(0) else 0
    val cols: Int get() = if (buffer.limit() >= 16) buffer.getInt(4) else 0
    val totalRows: Int get() = if (buffer.limit() >= 16) buffer.getInt(8) else 0
    val totalCols: Int get() = if (buffer.limit() >= 16) buffer.getInt(12) else 0

    private fun cellOffset(row: Int, col: Int): Int = 16 + (row * cols + col) * 24

    fun numericValue(row: Int, col: Int): Double = buffer.getDouble(cellOffset(row, col))
    fun measureId(row: Int, col: Int): Int = buffer.getInt(cellOffset(row, col) + 8)
    fun rowItemId(row: Int, col: Int): Int = buffer.getInt(cellOffset(row, col) + 12)
    fun colItemId(row: Int, col: Int): Int = buffer.getInt(cellOffset(row, col) + 16)
    fun cellTag(row: Int, col: Int): Byte = buffer.get(cellOffset(row, col) + 20)
    fun isDerived(row: Int, col: Int): Boolean = buffer.get(cellOffset(row, col) + 21).toInt() != 0
}
