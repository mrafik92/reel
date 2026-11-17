package app.reel.android

import org.junit.Assert.assertEquals
import org.junit.Test

class DeviceNameTest {
    @Test
    fun prefixesTheMakerOnce() {
        assertEquals("Google Pixel 7", AppGraph.deviceName("Google", "Pixel 7"))
        assertEquals("Xiaomi MiTV-AXSO0", AppGraph.deviceName("xiaomi", "MiTV-AXSO0"))
        assertEquals("NVIDIA SHIELD Android TV", AppGraph.deviceName("NVIDIA", "SHIELD Android TV"))
        assertEquals("Samsung SM-S918B", AppGraph.deviceName("samsung", "SM-S918B"))
        assertEquals("Amazon AFTMM", AppGraph.deviceName("Amazon", "AFTMM"))
    }
}
