import 'dart:convert';

import 'package:flutter/services.dart';

class HomeEndpoint {
  const HomeEndpoint({
    required this.uri,
    required this.serverId,
    required this.serverKey,
  });
  final Uri uri;
  final String serverId;

  /// DER-encoded SubjectPublicKeyInfo in unpadded base64url from the QR code.
  final String serverKey;

  String get _nativeKey =>
      base64.encode(base64Url.decode(base64Url.normalize(serverKey)));
}

class HomeRequest {
  const HomeRequest({this.method = 'GET', this.headers = const {}, this.body});
  final String method;
  final Map<String, String> headers;
  final Uint8List? body;
}

class HomeResponse {
  const HomeResponse(this.status, this.body);
  final int status;
  final Uint8List body;
}

class HomeTransport {
  static const _channel = MethodChannel('tube2pod/home_transport');
  Future<HomeResponse> request(
    HomeEndpoint endpoint,
    HomeRequest request,
  ) async {
    final response = await _channel.invokeMapMethod<String, dynamic>(
      'request',
      {
        'uri': endpoint.uri.toString(),
        'serverId': endpoint.serverId,
        'serverKey': endpoint._nativeKey,
        'method': request.method,
        'headers': request.headers,
        'body': request.body,
      },
    );
    return HomeResponse(
      response!['status'] as int,
      response['body'] as Uint8List,
    );
  }
}

class HomeMediaSource {
  const HomeMediaSource.paired(this.endpoint, {this.headers = const {}})
    : enclosure = null;
  const HomeMediaSource.enclosure(this.enclosure)
    : endpoint = null,
      headers = const {};
  final HomeEndpoint? endpoint;
  final Uri? enclosure;
  final Map<String, String> headers;
}

class HomePlayer {
  static const _channel = MethodChannel('tube2pod/home_transport');
  Future<void> open(HomeMediaSource source) => _channel.invokeMethod('open', {
    'uri': (source.endpoint?.uri ?? source.enclosure!).toString(),
    'serverKey': source.endpoint?._nativeKey,
    'headers': source.headers,
  });
  Future<void> play() => _channel.invokeMethod('play');
  Future<void> pause() => _channel.invokeMethod('pause');
  Future<void> seek(Duration position) =>
      _channel.invokeMethod('seek', position.inMilliseconds);
  Future<Map<String, dynamic>?> position() =>
      _channel.invokeMapMethod<String, dynamic>('position');
  Future<void> close() => _channel.invokeMethod('close');
}
