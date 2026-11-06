package dev.agentddp.sample

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * For `ddp test`: two tests that pass, one of them through UiAutomation (ddp's own runner holds it until `ddp test`
 * frees it), and a failure and a crash that run only when asked (`ddp test --env fail=1`, `--env crash=1`).
 */
@RunWith(AndroidJUnit4::class)
class SampleTest {
    private val asked = InstrumentationRegistry.getArguments()

    @Test
    fun packageName() {
        assertEquals("dev.agentddp.sample", InstrumentationRegistry.getInstrumentation().targetContext.packageName)
    }

    @Test
    fun uiAutomationIsFree() {
        assertNotNull(InstrumentationRegistry.getInstrumentation().uiAutomation)
    }

    @Test
    fun failsWhenAsked() {
        assumeTrue(asked.getString("fail") == "1")
        assertEquals("expected", "actual")
    }

    @Test
    fun crashesWhenAsked() {
        assumeTrue(asked.getString("crash") == "1")
        android.os.Process.killProcess(android.os.Process.myPid())
    }
}
