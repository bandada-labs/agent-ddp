package dev.agentddp

import org.json.JSONArray
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

    @Test
    fun attachingAnnouncesTheSessionBeforeAnswering() {
        val sent = messagesAfter("{\"id\":3,\"method\":\"Target.attachToTarget\",\"params\":{\"targetId\":\"app\",\"flatten\":true}}")
        assertEquals("Target.attachedToTarget", sent.first().getString("method"))
        val announced = sent.first().getJSONObject("params").getString("sessionId")
        assertEquals(announced, sent.last().getJSONObject("result").getString("sessionId"))
    }

    @Test
    fun onlyTheBrowserTargetAutoAttachesThePage() {
        val autoAttach = "{\"id\":1,\"method\":\"Target.setAutoAttach\",\"params\":{\"autoAttach\":true,\"flatten\":true}}"
        assertEquals(2, messagesAfter(autoAttach, browserLevel = true).size)
        assertEquals(1, messagesAfter(autoAttach, browserLevel = false).size)
    }

    @Test
    fun theAccessibilityTreeHangsTheElementsFromOneRoot() {
        val elements = JSONArray().put(JSONObject().put("label", "Tap me").put("role", "button")).put(JSONObject().put("id", "flashBox"))
        val nodes = Cdp.axTree("Example", elements)
        assertEquals(3, nodes.length())
        assertEquals("[\"2\",\"3\"]", nodes.getJSONObject(0).getJSONArray("childIds").toString())
        assertEquals("button", nodes.getJSONObject(1).getJSONObject("role").getString("value"))
        assertEquals("Tap me", nodes.getJSONObject(1).getJSONObject("name").getString("value"))
        assertEquals("flashBox", nodes.getJSONObject(2).getJSONObject("name").getString("value"))
        assertEquals("1", nodes.getJSONObject(2).getString("parentId"))
    }

    @Test
    fun selectorsMatchRoleIdAndLabel() {
        val dom = Dom("Example", JSONArray()
            .put(JSONObject().put("role", "button").put("id", "saveButton").put("label", "Save"))
            .put(JSONObject().put("role", "StaticText").put("label", "Saved at 10:00"))
            .put(JSONObject().put("role", "button").put("label", "Cancel")))
        assertEquals(listOf(4, 6), dom.query("button"))
        assertEquals(listOf(4), dom.query("button#saveButton"))
        assertEquals(listOf(4), dom.query("[label='Save']"))
        assertEquals(listOf(5), dom.query("text[label^=Saved]"))
        assertEquals(listOf(4, 6), dom.query("[label*=\"ance\"], #saveButton"))
        assertEquals(listOf(4), dom.query("[id]"))
        assertTrue(runCatching { dom.query("body button") }.isFailure)
        assertEquals("<button role=\"button\" id=\"saveButton\" label=\"Save\">Save</button>", dom.outerHTML(4))
    }

    @Test
    fun theDocumentStopsAtTheAskedDepth() {
        val dom = Dom("Example", JSONArray().put(JSONObject().put("role", "button")))
        val html = dom.document(1).getJSONArray("children").getJSONObject(0)
        assertEquals("HTML", html.getString("nodeName"))
        assertFalse(html.has("children"))
        assertTrue(dom.document(-1).toString().contains("BUTTON"))
    }

    @Test
    fun theBoxModelIsTheFrameInDp() {
        val dom = Dom("Example", JSONArray().put(JSONObject().put("frame", JSONArray(listOf(20, 40, 60, 80)))), density = 2f)
        assertEquals("[10,20,40,20,40,60,10,60]", dom.boxModel(4).getJSONArray("content").toString())
        assertTrue(runCatching { Dom("Example", JSONArray()).boxModel(4) }.isFailure)
    }

    private fun messagesAfter(message: String, browserLevel: Boolean = true): List<JSONObject> {
        val sent = mutableListOf<JSONObject>()
        CdpSession(CdpTarget("com.example", "Example"), browserLevel) { sent += JSONObject(it) }.receive(message)
        return sent
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
