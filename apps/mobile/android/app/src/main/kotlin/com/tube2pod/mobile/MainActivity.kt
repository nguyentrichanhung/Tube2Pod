package com.tube2pod.mobile

import android.media.MediaDataSource
import android.media.MediaPlayer
import android.os.Handler
import android.os.Looper
import android.util.Base64
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import java.net.URL
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.HttpsURLConnection
import javax.net.ssl.SSLContext
import javax.net.ssl.X509TrustManager

class MainActivity : FlutterActivity() {
    private var player: MediaPlayer? = null
    private val main = Handler(Looper.getMainLooper())

    override fun configureFlutterEngine(engine: FlutterEngine) {
        super.configureFlutterEngine(engine)
        MethodChannel(engine.dartExecutor.binaryMessenger, "tube2pod/home_transport").setMethodCallHandler { call, result ->
            if (call.method == "seek") {
                player?.seekTo((call.arguments as Number).toInt())
                result.success(null)
            } else if (call.method == "position") {
                result.success(mapOf("position" to player?.currentPosition, "duration" to player?.duration, "playing" to player?.isPlaying))
            } else if (call.method == "close") {
                player?.release()
                player = null
                result.success(null)
            } else if (call.method == "request" || call.method == "open") {
                val args = call.arguments as Map<*, *>
                Thread {
                    try {
                        val uri = args["uri"] as String
                        val pin = args["serverKey"] as String?
                        val value = if (call.method == "request") {
                            require(pin != null)
                            val connection = connect(uri, pin, args["method"] as String, args["headers"] as Map<*, *>)
                            val body = args["body"] as ByteArray?
                            if (body != null) { connection.doOutput = true; connection.outputStream.use { it.write(body) } }
                            val status = connection.responseCode
                            val bytes = (if (status >= 400) connection.errorStream else connection.inputStream)?.use { it.readBytes() } ?: byteArrayOf()
                            connection.disconnect()
                            mapOf("status" to status, "body" to bytes)
                        } else {
                            val media = MediaPlayer()
                            if (pin == null) {
                                require(URL(uri).protocol == "https")
                                media.setDataSource(uri)
                            } else {
                                media.setDataSource(RangeSource(uri, pin))
                            }
                            media.prepare()
                            media.start()
                            main.post { player?.release(); player = media }
                            null
                        }
                        main.post { result.success(value) }
                    } catch (e: Exception) {
                        val mismatch = generateSequence(e as Throwable?) { it.cause }.any { it is CertificateException && it.message == "identityMismatch" }
                        main.post { result.error(if (mismatch) "identityMismatch" else "transportError", e.message, null) }
                    }
                }.start()
            } else result.notImplemented()
        }
    }

    private fun connect(uri: String, pin: String, method: String, headers: Map<*, *>): HttpsURLConnection {
        val url = URL(uri)
        require(url.protocol == "https")
        val expected = Base64.decode(pin, Base64.DEFAULT)
        require(expected.isNotEmpty())
        val trust = object : X509TrustManager {
            override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
            override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) { throw CertificateException("client certificate unsupported") }
            override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
                if (chain.isEmpty() || !MessageDigest.isEqual(chain[0].publicKey.encoded, expected)) throw CertificateException("identityMismatch")
            }
        }
        val context = SSLContext.getInstance("TLS")
        context.init(null, arrayOf(trust), null)
        return (url.openConnection() as HttpsURLConnection).apply {
            sslSocketFactory = context.socketFactory
            hostnameVerifier = javax.net.ssl.HostnameVerifier { _, _ -> true }
            instanceFollowRedirects = false
            requestMethod = method
            headers.forEach { (key, value) -> setRequestProperty(key as String, value as String) }
        }
    }

    private inner class RangeSource(private val uri: String, private val pin: String) : MediaDataSource() {
        private val length: Long = connect(uri, pin, "HEAD", emptyMap<String, String>()).let {
            try { check(it.responseCode == 200); it.contentLengthLong.also { size -> check(size >= 0) } } finally { it.disconnect() }
        }
        override fun getSize() = length
        override fun readAt(position: Long, buffer: ByteArray, offset: Int, size: Int): Int {
            if (size == 0) return 0
            if (position >= length) return -1
            val end = minOf(length - 1, position + size - 1)
            val connection = connect(uri, pin, "GET", mapOf("Range" to "bytes=$position-$end"))
            try {
                check(connection.responseCode == 206)
                return connection.inputStream.use { it.read(buffer, offset, size) }
            } finally { connection.disconnect() }
        }
        override fun close() {}
    }
}
