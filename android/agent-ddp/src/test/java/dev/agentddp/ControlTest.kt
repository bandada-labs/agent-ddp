package dev.agentddp

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit

class ControlTest {
    @Test
    fun acceptKeyMatchesTheRfcExample() {
        assertEquals("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=", WebSocket.acceptKey("dGhlIHNhbXBsZSBub25jZQ=="))
    }

    @Test
    fun aMaskedFragmentedMessageReadsAsOne() {
        val stream = ByteArrayOutputStream()
        clientFrame(stream, fin = false, opcode = WebSocket.TEXT, text = "{\"id\":")
        clientFrame(stream, fin = true, opcode = WebSocket.CONTINUATION, text = "1}")
        val frame = WebSocket.readFrame(ByteArrayInputStream(stream.toByteArray()))
        assertEquals(WebSocket.TEXT, frame.opcode)
        assertEquals("{\"id\":1}", String(frame.payload))
    }

    @Test
    fun aLongServerFrameReadsBack() {
        val text = "x".repeat(70_000)
        val stream = ByteArrayOutputStream()
        WebSocket.writeText(stream, text)
        assertEquals(text, String(WebSocket.readFrame(ByteArrayInputStream(stream.toByteArray())).payload))
    }

    @Test
    fun anUnknownMethodAnswersWithItsName() {
        val answer = answerTo("{\"id\":7,\"method\":\"foo.bar\"}")
        assertEquals(7, answer.getInt("id"))
        assertEquals(-32601, answer.getJSONObject("error").getInt("code"))
        assertEquals("Unknown method 'foo.bar'", answer.getJSONObject("error").getString("message"))
    }

    @Test
    fun aTouchNeedsItsPhase() {
        val error = answerTo("""{"id":1,"method":"input.touch","params":{"x":1,"y":1}}""").getJSONObject("error")
        assertEquals("phase is down, move or up", error.getString("message"))
    }

    @Test
    fun aLogcatLineBecomesALogEntry() {
        val entry = AppLog.logEntry("1729700000.123  8684  8700 W DDPSample: slow frame")!!
        assertEquals("""{"t":1729700000123,"level":"warn","tag":"DDPSample","text":"slow frame"}""", entry.toString())
        assertEquals(null, AppLog.logEntry("--------- beginning of main"))
    }

    @Test
    fun `a paused request shows redacted and goes on with its real secret`() {
        val paused = ArrayBlockingQueue<JSONObject>(1)
        val session = ControlSession(ControlApp("com.example", "Example")) { text ->
            JSONObject(text).takeIf { it.optString("method") == "net.paused" }?.let { paused.put(it.getJSONObject("params")) }
        }
        session.receive("""{"id":1,"method":"net.intercept","params":{"urls":["*/items\\?*"]}}""")
        val decision = ArrayBlockingQueue<Fetch.Decision>(1)
        Thread {
            decision.put(Fetch.pause(Fetch.Sent("http://127.0.0.1/items?page=1", "GET", mapOf("authorization" to "Bearer real"), null)))
        }.start()
        val event = paused.poll(2, TimeUnit.SECONDS)!!
        assertEquals("[redacted]", event.getJSONObject("request").getJSONObject("headers").getString("authorization"))
        session.receive("""{"id":2,"method":"net.continue","params":{"id":"${event.getString("id")}",""" +
            """"headers":{"authorization":"[redacted]","x-extra":"1"}}}""")
        val sent = (decision.poll(2, TimeUnit.SECONDS) as Fetch.Decision.Proceed).request
        assertEquals(mapOf("authorization" to "Bearer real", "x-extra" to "1"), sent.headers)
        assertFalse(Fetch.wildcard("*/items\\?*").matches("http://a/itemsX"))
        session.close()
    }

    @Test
    fun aFailedRequestThrowsWhatOkHttpWould() {
        val paused = ArrayBlockingQueue<String>(1)
        val session = ControlSession(ControlApp("com.example", "Example")) { text ->
            JSONObject(text).optJSONObject("params")?.optString("id")?.takeIf { it.isNotEmpty() }?.let(paused::put)
        }
        session.receive("""{"id":1,"method":"net.intercept","params":{}}""")
        val decision = ArrayBlockingQueue<Fetch.Decision>(1)
        Thread { decision.put(Fetch.pause(Fetch.Sent("http://127.0.0.1/a", "GET", emptyMap(), null))) }.start()
        val id = paused.poll(2, TimeUnit.SECONDS)!!
        session.receive("""{"id":2,"method":"net.fail","params":{"id":"$id","error":"offline"}}""")
        val failed = decision.poll(2, TimeUnit.SECONDS) as Fetch.Decision.Fail
        assertEquals(java.net.UnknownHostException::class, failed.error::class)
        session.close()
    }

    @Test
    fun aStringSetShowsAsJsonAndReadsBack() {
        val shown = Preferences.shown(setOf("b", "a"))
        assertEquals("[\"a\",\"b\"]", shown)
        assertEquals(setOf("a", "b"), Preferences.stringSet(shown))
        assertEquals(null, Preferences.stringSet("[1]"))
        assertEquals("3", Preferences.shown(3))
    }

    @Test
    fun aCommandRunsOffTheReadingThreadAndAnswersItsValue() {
        val reader = Thread.currentThread()
        AgentDdpCommands.add("login") { params -> mapOf("user" to params.getString("user"), "apart" to (Thread.currentThread() != reader)) }
        val answers = ArrayBlockingQueue<JSONObject>(2)
        val session = ControlSession(ControlApp("com.example", "Example")) { answers.put(JSONObject(it)) }
        session.receive("""{"id":1,"method":"app.run","params":{"name":"login","params":{"user":"test"}}}""")
        assertEquals("""{"user":"test","apart":true}""", answers.poll(2, TimeUnit.SECONDS)!!.getJSONObject("result").getJSONObject("value").toString())
        session.receive("""{"id":2,"method":"app.run","params":{"name":"nope"}}""")
        assertEquals("No command 'nope'; app.commands lists them", answers.poll(2, TimeUnit.SECONDS)!!.getJSONObject("error").getString("message"))
        assertEquals("[\"login\"]", answerTo("""{"id":3,"method":"app.commands"}""").getJSONObject("result").getJSONArray("commands").toString())
    }

    @Test
    fun aCommandThatReturnsNoJsonSaysSo() {
        AgentDdpCommands.add("date") { java.util.Date() }
        val answers = ArrayBlockingQueue<JSONObject>(1)
        ControlSession(ControlApp("com.example", "Example")) { answers.put(JSONObject(it)) }
            .receive("""{"id":1,"method":"app.run","params":{"name":"date"}}""")
        assertEquals("'date' returned a Date, not JSON", answers.poll(2, TimeUnit.SECONDS)!!.getJSONObject("error").getString("message"))
    }

    @Test
    fun netSendWithoutOkHttpInTheAppSaysSo() {
        // Unit tests run without OkHttp, as an app without it would.
        val answers = ArrayBlockingQueue<JSONObject>(1)
        ControlSession(ControlApp("com.example", "Example")) { answers.put(JSONObject(it)) }
            .receive("""{"id":1,"method":"net.send","params":{"url":"http://127.0.0.1/"}}""")
        assertEquals("net.send needs OkHttp in the app", answers.poll(2, TimeUnit.SECONDS)!!.getJSONObject("error").getString("message"))
    }

    private fun answerTo(message: String): JSONObject {
        var answer = ""
        ControlSession(ControlApp("com.example", "Example")) { answer = it }.receive(message)
        return JSONObject(answer)
    }

    private fun clientFrame(stream: ByteArrayOutputStream, fin: Boolean, opcode: Int, text: String) {
        val mask = byteArrayOf(1, 2, 3, 4)
        val payload = text.toByteArray()
        stream.write((if (fin) 0x80 else 0) or opcode)
        stream.write(0x80 or payload.size)
        stream.write(mask)
        stream.write(ByteArray(payload.size) { (payload[it].toInt() xor mask[it % 4].toInt()).toByte() })
    }
}
