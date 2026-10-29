package dev.agentddp.sample

import dev.agentddp.AgentDdpCommands
import dev.agentddp.AgentDdpEvents
import dev.agentddp.AgentDdpState
import android.os.Handler
import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import javax.crypto.KeyGenerator
import java.io.File
import android.os.Looper

/** The sample's own values for `ddp state`: a mode an agent can set, and how many loads ran. */
object SampleState {
    var mode = "normal"
    var loads = 0
}

/** Debug: what an agent can ask the sample to do (`app.commands`, `app.run`) and read (`state.*`). */
fun addSampleCommands(context: Context) {
    // For `ddp storage`: a database, a text file and a Keystore key to read.
    AgentDdpCommands.add("seed") {
        File(context.filesDir, "note.txt").writeText("hello from the sample\n")
        SQLiteDatabase.openOrCreateDatabase(context.getDatabasePath("sample.db").apply { parentFile?.mkdirs() }, null).use { db ->
            db.execSQL("CREATE TABLE IF NOT EXISTS items(name TEXT, token TEXT)")
            db.execSQL("DELETE FROM items")
            db.execSQL("INSERT INTO items VALUES ('first', 'abc123'), ('second', 'def456')")
        }
        KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder("demo-key", KeyProperties.PURPOSE_ENCRYPT).build())
        }.generateKey()
        "seeded"
    }
    AgentDdpCommands.add("load") {
        SampleNetwork.load().also { answer ->
            SampleState.loads++
            AgentDdpEvents.send("loaded", mapOf("loads" to SampleState.loads, "answer" to answer))
        }
    }
    // For `ddp audits`: StrictMode on, logging only, then a disk read on the main thread it reports.
    AgentDdpCommands.add("warn") {
        Handler(Looper.getMainLooper()).post {
            android.os.StrictMode.setThreadPolicy(android.os.StrictMode.ThreadPolicy.Builder().detectDiskReads().penaltyLog().build())
            File(context.filesDir, "note.txt").exists().also { File(context.filesDir, "probe").readBytesOrNull() }
        }
        null
    }
    // For `ddp perf`: the main thread busy for 200 ms, about 11 refreshes missed at 60 Hz.
    AgentDdpCommands.add("jank") {
        Handler(Looper.getMainLooper()).post { Thread.sleep(200) }
        null
    }
    // For `ddp crash`: an uncaught exception on the main thread.
    AgentDdpCommands.add("crash") {
        Handler(Looper.getMainLooper()).post { throw IllegalStateException("the sample crashed on purpose") }
        null
    }
    AgentDdpState.add("mode", get = { SampleState.mode }, set = { SampleState.mode = it as? String ?: SampleState.mode })
    AgentDdpState.add("loads", get = { SampleState.loads })
}

private fun File.readBytesOrNull(): ByteArray? = runCatching { readBytes() }.getOrNull()
