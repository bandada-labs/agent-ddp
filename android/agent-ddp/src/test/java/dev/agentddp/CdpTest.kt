package dev.agentddp

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream

class CdpTest {
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
    fun anUnknownMethodAnswersLikeChrome() {
        val answer = answerTo("{\"id\":7,\"method\":\"Foo.bar\"}")
        assertEquals(7, answer.getInt("id"))
        assertEquals(-32601, answer.getJSONObject("error").getInt("code"))
        assertEquals("'Foo.bar' wasn't found", answer.getJSONObject("error").getString("message"))
    }

    @Test
    fun aKnownMethodAnswersWithItsResultAndSession() {
        val answer = answerTo("{\"id\":1,\"method\":\"Target.getTargets\",\"sessionId\":\"s1\"}")
        assertEquals("app", answer.getJSONObject("result").getJSONArray("targetInfos").getJSONObject(0).getString("targetId"))
        assertEquals("s1", answer.getString("sessionId"))
    }

    @Test
    fun webPagesMayNotConnect() {
        assertTrue(Cdp.originAllowed(null))
        assertTrue(Cdp.originAllowed("devtools://devtools"))
        assertFalse(Cdp.originAllowed("http://localhost:3000"))
    }

    private fun answerTo(message: String): JSONObject {
        var answer = ""
        CdpSession(CdpTarget("com.example", "Example")) { answer = it }.receive(message)
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
