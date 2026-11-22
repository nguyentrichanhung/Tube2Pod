import Flutter
import UIKit
import Security
import AVFoundation

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  private var player: AVPlayer?
  private var loader: PinnedMediaLoader?
  private var playerObservation: NSKeyValueObservation?
  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
    let channel = FlutterMethodChannel(name: "tube2pod/home_transport", binaryMessenger: engineBridge.applicationRegistrar.messenger())
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self = self else { return }
      if call.method == "close" {
        self.player?.pause()
        self.player = nil
        self.loader = nil
        self.playerObservation = nil
        result(nil)
        return
      }
      if call.method == "seek", let millis = call.arguments as? Int {
        self.player?.seek(to: CMTime(value: CMTimeValue(millis), timescale: 1000))
        result(nil)
        return
      }
      if call.method == "position" {
        let position = self.player?.currentTime().seconds ?? 0
        let duration = self.player?.currentItem?.duration.seconds ?? 0
        result(["position": position.isFinite ? Int(position * 1000) : 0,
                "duration": duration.isFinite ? Int(duration * 1000) : 0,
                "playing": self.player?.rate != 0])
        return
      }
      if call.method == "open", let args = call.arguments as? [String: Any],
         let address = args["uri"] as? String, let url = URL(string: address), url.scheme == "https" {
        let asset: AVURLAsset
        if let encoded = args["serverKey"] as? String, let pin = Data(base64Encoded: encoded), !pin.isEmpty {
          var components = URLComponents(url: url, resolvingAgainstBaseURL: false)!
          components.scheme = "tube2pod"
          asset = AVURLAsset(url: components.url!)
          let loader = PinnedMediaLoader(url: url, pin: pin)
          asset.resourceLoader.setDelegate(loader, queue: .main)
          self.loader = loader
        } else if args["serverKey"] is NSNull || args["serverKey"] == nil {
          self.loader = nil
          asset = AVURLAsset(url: url)
        } else {
          result(FlutterError(code: "transportError", message: "Invalid server key", details: nil))
          return
        }
        self.player?.pause()
        let item = AVPlayerItem(asset: asset)
        self.player = AVPlayer(playerItem: item)
        self.playerObservation = item.observe(\.status, options: [.initial, .new]) { [weak self] item, _ in
          guard item.status != .unknown else { return }
          DispatchQueue.main.async {
            self?.playerObservation = nil
            if item.status == .readyToPlay {
              self?.player?.play()
              result(nil)
            } else {
              result(FlutterError(code: "transportError", message: item.error?.localizedDescription, details: nil))
            }
          }
        }
        return
      }
      guard call.method == "request", let args = call.arguments as? [String: Any],
            let address = args["uri"] as? String, let url = URL(string: address), url.scheme == "https",
            let encoded = args["serverKey"] as? String, let pin = Data(base64Encoded: encoded), !pin.isEmpty,
            let method = args["method"] as? String else {
        result(FlutterMethodNotImplemented)
        return
      }
      var request = URLRequest(url: url)
      request.httpMethod = method
      (args["headers"] as? [String: String])?.forEach { request.setValue($0.value, forHTTPHeaderField: $0.key) }
      request.httpBody = (args["body"] as? FlutterStandardTypedData)?.data
      let verifier = PinnedSession(pin: pin)
      let session = URLSession(configuration: .ephemeral, delegate: verifier, delegateQueue: nil)
      session.dataTask(with: request) { data, response, error in
        DispatchQueue.main.async {
          defer { session.finishTasksAndInvalidate() }
          if verifier.mismatch {
            result(FlutterError(code: "identityMismatch", message: "Server key does not match", details: nil))
          } else if let error = error {
            result(FlutterError(code: "transportError", message: error.localizedDescription, details: nil))
          } else if let response = response as? HTTPURLResponse {
            result(["status": response.statusCode, "body": FlutterStandardTypedData(bytes: data ?? Data())])
          } else {
            result(FlutterError(code: "transportError", message: "Invalid response", details: nil))
          }
        }
      }.resume()
    }
  }
}

private final class PinnedSession: NSObject, URLSessionDelegate, URLSessionTaskDelegate {
  let pin: Data
  var mismatch = false
  init(pin: Data) { self.pin = pin }

  func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge,
                  completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
    guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
          let trust = challenge.protectionSpace.serverTrust,
          let cert = SecTrustGetCertificateAtIndex(trust, 0),
          let spki = certificateSPKI(SecCertificateCopyData(cert) as Data), spki == pin else {
      mismatch = true
      completionHandler(.cancelAuthenticationChallenge, nil)
      return
    }
    completionHandler(.useCredential, URLCredential(trust: trust))
  }

  func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                  newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
    completionHandler(nil)
  }
}

// Extract the full DER SubjectPublicKeyInfo element from the leaf certificate.
private func certificateSPKI(_ certificate: Data) -> Data? {
  let bytes = [UInt8](certificate)
  func element(_ offset: inout Int, limit: Int) -> (Int, Int, Int)? {
    let start = offset
    guard offset + 2 <= limit else { return nil }
    let tag = Int(bytes[offset]); offset += 1
    let first = Int(bytes[offset]); offset += 1
    var length = first
    if first & 0x80 != 0 {
      let count = first & 0x7f
      guard count > 0, count <= 4, offset + count <= limit else { return nil }
      length = 0
      for _ in 0..<count { length = length * 256 + Int(bytes[offset]); offset += 1 }
    }
    guard length <= limit - offset else { return nil }
    return (tag, start, offset + length)
  }
  var offset = 0
  guard let outer = element(&offset, limit: bytes.count), outer.0 == 0x30,
        let tbs = element(&offset, limit: outer.2), tbs.0 == 0x30 else { return nil }
  if offset < tbs.2 && bytes[offset] == 0xa0 {
    guard let version = element(&offset, limit: tbs.2) else { return nil }
    offset = version.2
  }
  for _ in 0..<5 {
    guard let field = element(&offset, limit: tbs.2) else { return nil }
    offset = field.2
  }
  guard let spki = element(&offset, limit: tbs.2), spki.0 == 0x30 else { return nil }
  return Data(bytes[spki.1..<spki.2])
}


private final class PinnedMediaLoader: NSObject, AVAssetResourceLoaderDelegate {
  private let url: URL
  private let verifier: PinnedSession
  private lazy var session = URLSession(configuration: .ephemeral, delegate: verifier, delegateQueue: nil)
  private var tasks: [AVAssetResourceLoadingRequest: URLSessionDataTask] = [:]

  init(url: URL, pin: Data) {
    self.url = url
    self.verifier = PinnedSession(pin: pin)
  }

  func resourceLoader(_ resourceLoader: AVAssetResourceLoader, shouldWaitForLoadingOfRequestedResource loadingRequest: AVAssetResourceLoadingRequest) -> Bool {
    guard let dataRequest = loadingRequest.dataRequest else {
      var request = URLRequest(url: url)
      request.httpMethod = "HEAD"
      let task = session.dataTask(with: request) { [weak self] _, response, error in
        DispatchQueue.main.async {
          self?.tasks.removeValue(forKey: loadingRequest)
          guard let http = response as? HTTPURLResponse, http.statusCode == 200,
                http.expectedContentLength >= 0, error == nil else {
            loadingRequest.finishLoading(with: error ?? NSError(domain: "HomeTransport", code: 1))
            return
          }
          loadingRequest.contentInformationRequest?.contentType = "public.mp3"
          loadingRequest.contentInformationRequest?.contentLength = http.expectedContentLength
          loadingRequest.contentInformationRequest?.isByteRangeAccessSupported = true
          loadingRequest.finishLoading()
        }
      }
      tasks[loadingRequest] = task
      task.resume()
      return true
    }
    let start = max(dataRequest.requestedOffset, dataRequest.currentOffset)
    let length = Int64(dataRequest.requestedLength) - (start - dataRequest.requestedOffset)
    guard length > 0 else { loadingRequest.finishLoading(); return true }
    var request = URLRequest(url: url)
    request.setValue("bytes=\(start)-\(start + length - 1)", forHTTPHeaderField: "Range")
    let task = session.dataTask(with: request) { [weak self] data, response, error in
      DispatchQueue.main.async {
        self?.tasks.removeValue(forKey: loadingRequest)
        guard let http = response as? HTTPURLResponse, http.statusCode == 206,
              let range = http.value(forHTTPHeaderField: "Content-Range"),
              let total = Int64(range.split(separator: "/").last ?? ""), let data = data, error == nil else {
          loadingRequest.finishLoading(with: error ?? NSError(domain: "HomeTransport", code: 1))
          return
        }
        loadingRequest.contentInformationRequest?.contentType = "public.mp3"
        loadingRequest.contentInformationRequest?.contentLength = total
        loadingRequest.contentInformationRequest?.isByteRangeAccessSupported = true
        dataRequest.respond(with: data)
        loadingRequest.finishLoading()
      }
    }
    tasks[loadingRequest] = task
    task.resume()
    return true
  }

  func resourceLoader(_ resourceLoader: AVAssetResourceLoader, didCancel loadingRequest: AVAssetResourceLoadingRequest) {
    tasks.removeValue(forKey: loadingRequest)?.cancel()
  }

  deinit { session.invalidateAndCancel() }
}
