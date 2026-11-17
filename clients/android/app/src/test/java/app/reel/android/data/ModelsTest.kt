package app.reel.android.data

import java.io.File
import kotlinx.serialization.KSerializer
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ModelsTest {
    /** Model → its schema in server/openapi.json. Add every new wire model here. */
    private val schemas: Map<KSerializer<*>, String> = mapOf(
        Health.serializer() to "Health",
        SetupStatus.serializer() to "SetupStatus",
        LoginBody.serializer() to "LoginBody",
        Permissions.serializer() to "PermissionsDto",
        Me.serializer() to "Me",
        ErrorBody.serializer() to "ErrorBody",
        LoginResponse.serializer() to "LoginResponse",
        ItemSummary.serializer() to "ItemSummary",
        HomeRow.serializer() to "HomeRow",
        Home.serializer() to "Home",
        Library.serializer() to "Library",
        ItemPage.serializer() to "ItemPage",
        AudioStream.serializer() to "AudioStream",
        SubtitleStream.serializer() to "SubtitleStream",
        VideoStream.serializer() to "VideoStream",
        MediaFile.serializer() to "MediaFile",
        CastMember.serializer() to "CastMember",
        ItemDetail.serializer() to "ItemDetail",
        DecideRequest.serializer() to "DecideRequest",
        SubtitleOption.serializer() to "SubtitleOption",
        Selected.serializer() to "Selected",
        DecideResponse.serializer() to "DecideResponse",
        ProgressRequest.serializer() to "ProgressRequest",
        IptvChannel.serializer() to "IptvChannel",
        Program.serializer() to "Program",
        NowNext.serializer() to "NowNext",
        TuneBody.serializer() to "TuneBody",
        TuneResponse.serializer() to "TuneResponse",
        GenreCount.serializer() to "GenreCount",
        PersonHit.serializer() to "PersonHit",
        SearchAll.serializer() to "SearchAll",
    )

    private val openapi: JsonObject by lazy {
        val path = System.getProperty("reel.openapi") ?: "../../../server/openapi.json"
        Json.parseToJsonElement(File(path).readText()).jsonObject
    }

    /** Properties and required names of a schema, following `allOf`. */
    private fun schema(name: String): Pair<Set<String>, Set<String>> {
        val s = openapi["components"]!!.jsonObject["schemas"]!!.jsonObject[name]?.jsonObject
            ?: error("schema $name is not in openapi.json")
        val parts = (s["allOf"] as? JsonArray)?.map { p ->
            val o = p.jsonObject
            o["\$ref"]?.jsonPrimitive?.content?.let { schema(it.substringAfterLast('/')) }
                ?: (o["properties"]?.jsonObject?.keys.orEmpty() to o["required"]?.jsonArray?.map { it.jsonPrimitive.content }.orEmpty().toSet())
        } ?: listOf(s["properties"]?.jsonObject?.keys.orEmpty() to s["required"]?.jsonArray?.map { it.jsonPrimitive.content }.orEmpty().toSet())
        return parts.flatMap { it.first }.toSet() to parts.flatMap { it.second }.toSet()
    }

    @Test
    fun modelsMatchTheServerSchemas() {
        val problems = mutableListOf<String>()
        for ((ser, name) in schemas) {
            val (props, required) = schema(name)
            val d = ser.descriptor
            for (i in 0 until d.elementsCount) {
                val field = d.getElementName(i)
                if (field !in props) problems += "$name.$field: not in the schema (typo, or removed on the server)"
                // A field the app can't do without must be one the server always sends.
                if (!d.isElementOptional(i) && !d.getElementDescriptor(i).isNullable && field !in required && !name.endsWith("Body") && !name.endsWith("Request")) {
                    problems += "$name.$field: required by the app but optional on the server; give it a default"
                }
            }
        }
        assertTrue(problems.joinToString("\n"), problems.isEmpty())
    }

    @Test
    fun parsesAnItemDetail() {
        val d = ReelJson.decodeFromString(
            ItemDetail.serializer(),
            """
            {"id":42,"kind":"episode","title":"Pilot","played":false,"added_at":1700000000,"year":2008,
             "parent_id":41,"season_no":1,"episode_no":1,"progress":0.25,"genres":["Drama"],
             "cast":[{"name":"A","character":"B","photo_url":null,"person_tmdb_id":5}],"children_count":0,
             "content_rating":"TV-14","overview":"x","rating":8.9,"runtime_s":3480,"tmdb_id":1396,
             "files":[{"id":9,"container":"mkv","duration_s":3480.5,"size":123,"url":"/api/v1/direct/9",
                       "video":{"codec":"hevc","width":3840,"height":2160,"hdr":"hdr10","bit_depth":10},
                       "audio":[{"index":1,"codec":"eac3","channels":6,"default":true,"language":"eng"}],
                       "subtitles":[{"index":2,"codec":"subrip","forced":false,"external":false}],
                       "some_future_field":{"nested":true}}]}
            """.trimIndent(),
        )
        assertTrue(d.playable)
        assertEquals("hdr10", d.files[0].video!!.hdr)
        assertEquals(6L, d.files[0].audio[0].channels)
        assertEquals(1396L, d.tmdbId)
    }

    @Test
    fun parsesHomeAndPages() {
        val h = ReelJson.decodeFromString(
            Home.serializer(),
            """{"continue_watching":[{"id":1,"kind":"movie","title":"M","played":false,"added_at":1,"poster_url":"/api/v1/images/1?sig=x"}],
                "recently_added":[],"rows":[{"library_id":2,"title":"Shows","items":[]}]}""",
        )
        assertEquals("/api/v1/images/1?sig=x", h.continueWatching[0].posterUrl)
        val p = ReelJson.decodeFromString(ItemPage.serializer(), """{"items":[],"next_cursor":"abc"}""")
        assertEquals("abc", p.nextCursor)
    }

    @Test
    fun parsesDecideAndTune() {
        val r = ReelJson.decodeFromString(
            DecideResponse.serializer(),
            """{"session_id":"s1","mode":"remux","url":"/api/v1/hls/s1/master.m3u8?sig=x","decision":{},"subtitles":[],
                "audio":[],"selected":{"reason":"ok"},"bitrate_kbps":8000,"quality":"original","quality_options":[],"variants":[]}""",
        )
        assertTrue(r.isHls)
        val t = ReelJson.decodeFromString(
            TuneResponse.serializer(),
            """{"session_id":"l1","mode":"passthrough","url":"/api/v1/iptv/sessions/l1/stream.ts","channel_id":3,"channel_name":"News","hls":null}""",
        )
        assertEquals("passthrough", t.mode)
    }

    @Test
    fun requestsLeaveOutNullsButKeepDefaults() {
        val s = ReelJson.encodeToString(DecideRequest.serializer(), DecideRequest(deviceId = "d", startS = 12.5))
        assertEquals("""{"preset":"android_exoplayer","device_id":"d","start_s":12.5}""", s)
        val l = ReelJson.encodeToString(LoginBody.serializer(), LoginBody("u", "p"))
        assertEquals("""{"username":"u","password":"p","client":"android"}""", l)
    }

    @Test
    fun meDefaultsAndPermissions() {
        val me = ReelJson.decodeFromString(Me.serializer(), """{"id":1,"username":"u","role":"user"}""")
        assertFalse(me.canWatchLive)
        assertNull(me.libraryIds)
    }
}
