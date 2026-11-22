import 'dart:convert';
import 'dart:io';

import 'package:flutter/services.dart';

import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:multicast_dns/multicast_dns.dart';

import '../../platform/home_transport.dart';

class PairEnvelopeV1 {
  PairEnvelopeV1(this.serverId, this.lanUri, this.serverKey, this.ticket);
  final String serverId, serverKey, ticket;
  final Uri lanUri;
  factory PairEnvelopeV1.parse(String raw) {
    final json = jsonDecode(raw) as Map<String, dynamic>;
    final uri = Uri.parse(json['lan_uri'] as String);
    final key = json['server_key'] as String;
    if (json['version'] != 1 ||
        uri.scheme != 'https' ||
        uri.host.isEmpty ||
        (json['server_id'] as String).isEmpty ||
        (json['ticket'] as String).isEmpty ||
        base64Url.decode(base64Url.normalize(key)).isEmpty ||
        (json['expires_at'] as num) <=
            DateTime.now().millisecondsSinceEpoch ~/ 1000) {
      throw const FormatException('Mã ghép không hợp lệ hoặc đã hết hạn');
    }
    return PairEnvelopeV1(
      json['server_id'] as String,
      uri,
      key,
      json['ticket'] as String,
    );
  }
}

enum ConnectionStatus { disconnected, lan, remote, revoked }

class ConnectionController extends ChangeNotifier {
  ConnectionController({
    HomeTransport? transport,
    FlutterSecureStorage? storage,
    this.discover,
  }) : transport = transport ?? HomeTransport(),
       storage = storage ?? const FlutterSecureStorage();
  final HomeTransport transport;
  final FlutterSecureStorage storage;
  final Future<List<Uri>> Function()? discover;
  ConnectionStatus status = ConnectionStatus.disconnected;
  String? serverId, serverKey, credential, adminUri, publicOrigin;
  Uri? lanUri, currentUri;
  Map<String, String> get headers => {'Authorization': 'Bearer $credential'};
  HomeEndpoint endpoint(String path) => HomeEndpoint(
    uri: currentUri!.resolve(path),
    serverId: serverId!,
    serverKey: serverKey!,
  );
  Future<dynamic> request(
    String path, {
    String method = 'GET',
    Object? body,
  }) async {
    final result = await transport.request(
      endpoint(path),
      HomeRequest(
        method: method,
        headers: {
          ...headers,
          if (body != null) 'Content-Type': 'application/json',
        },
        body: body == null
            ? null
            : Uint8List.fromList(utf8.encode(jsonEncode(body))),
      ),
    );
    if (result.status == 401) {
      status = ConnectionStatus.revoked;
      notifyListeners();
    }
    if (result.status < 200 || result.status >= 300) {
      throw StateError('HTTP ${result.status}');
    }
    return jsonDecode(utf8.decode(result.body));
  }

  Future<void> pair(PairEnvelopeV1 envelope) async {
    final response = await transport.request(
      HomeEndpoint(
        uri: envelope.lanUri.resolve('/v1/pair'),
        serverId: envelope.serverId,
        serverKey: envelope.serverKey,
      ),
      HomeRequest(
        method: 'POST',
        headers: const {'Content-Type': 'application/json'},
        body: Uint8List.fromList(
          utf8.encode(
            jsonEncode({'ticket': envelope.ticket, 'label': 'Điện thoại'}),
          ),
        ),
      ),
    );
    if (response.status != 200) {
      throw StateError(
        response.status == 409 || response.status == 410
            ? 'Mã đã dùng hoặc hết hạn. Tạo mã mới trên máy chủ.'
            : 'Không ghép được (HTTP ${response.status}).',
      );
    }
    final grant =
        jsonDecode(utf8.decode(response.body)) as Map<String, dynamic>;
    if (grant['server_id'] != envelope.serverId ||
        grant['credential'] is! String ||
        (grant['credential'] as String).isEmpty) {
      throw StateError('Danh tính máy chủ không khớp');
    }
    serverId = envelope.serverId;
    serverKey = envelope.serverKey;
    lanUri = envelope.lanUri;
    currentUri = lanUri;
    credential = grant['credential'] as String;
    await _save();
    await _check();
  }

  Future<void> _save() => storage.write(
    key: 'home',
    value: jsonEncode({
      'server_id': serverId,
      'server_key': serverKey,
      'lan_uri': lanUri.toString(),
      'admin_uri': adminUri,
      'credential': credential,
    }),
  );

  Future<void> _check({bool remote = false}) async {
    final data = await request('/v1/connection') as Map<String, dynamic>;
    if (data['server_id'] != serverId) {
      throw const FormatException('Danh tính máy chủ không khớp');
    }
    adminUri = data['admin_uri'] as String?;
    publicOrigin = data['public_origin'] as String?;
    await _save();
    status = remote ? ConnectionStatus.remote : ConnectionStatus.lan;
    notifyListeners();
  }

  Future<void> reconnect() async {
    if (credential == null) {
      final saved = await storage.read(key: 'home');
      if (saved == null) return;
      final data = jsonDecode(saved) as Map<String, dynamic>;
      serverId = data['server_id'] as String;
      serverKey = data['server_key'] as String;
      lanUri = Uri.parse(data['lan_uri'] as String);
      adminUri = data['admin_uri'] as String?;
      credential = data['credential'] as String;
    }
    final candidates = <Uri>[];
    final client = MDnsClient();
    try {
      if (discover != null) {
        candidates.addAll(await discover!());
      } else if (Platform.isIOS) {
        final found = await const MethodChannel('tube2pod/discovery')
            .invokeListMethod<String>('discover', serverId);
        candidates.addAll((found ?? []).map(Uri.parse));
      } else {
        if (Platform.isAndroid) {
          await const MethodChannel('tube2pod/discovery')
              .invokeMethod<void>('start');
        }
        await client.start();
        await for (final ptr in client.lookup<PtrResourceRecord>(
          ResourceRecordQuery.serverPointer('_tube2pod._tcp.local'),
          timeout: const Duration(seconds: 2),
        )) {
          if (ptr.domainName != '$serverId._tube2pod._tcp.local' &&
              ptr.domainName != '$serverId._tube2pod._tcp.local.') {
            continue;
          }
          await for (final srv in client.lookup<SrvResourceRecord>(
            ResourceRecordQuery.service(ptr.domainName),
            timeout: const Duration(seconds: 1),
          )) {
            await for (final ip in client.lookup<IPAddressResourceRecord>(
              ResourceRecordQuery.addressIPv4(srv.target),
              timeout: const Duration(seconds: 1),
            )) {
              candidates.add(
                Uri(scheme: 'https', host: ip.address.address, port: srv.port),
              );
            }
          }
        }
      }
    } catch (_) {
    } finally {
      client.stop();
      if (Platform.isAndroid) {
        try {
          await const MethodChannel('tube2pod/discovery')
              .invokeMethod<void>('stop');
        } catch (_) {}
      }
    }
    candidates.add(lanUri!);
    for (final uri in candidates) {
      currentUri = uri;
      try {
        await _check();
        if (uri != lanUri) {
          lanUri = uri;
          await _save();
        }
        return;
      } on PlatformException catch (e) {
        if (e.code == 'identityMismatch') {
          status = ConnectionStatus.disconnected;
          notifyListeners();
          rethrow;
        }
        if (e.code != 'transportError') rethrow;
      } on FormatException {
        status = ConnectionStatus.disconnected;
        notifyListeners();
        rethrow;
      } catch (_) {
        if (status == ConnectionStatus.revoked) return;
        rethrow;
      }
    }
    final remote = Uri.tryParse(adminUri ?? '');
    if (remote != null && remote.scheme == 'https' && remote.host.isNotEmpty) {
      currentUri = remote;
      try {
        await _check(remote: true);
        return;
      } on PlatformException catch (e) {
        if (e.code != 'transportError') {
          status = ConnectionStatus.disconnected;
          notifyListeners();
          rethrow;
        }
      } on FormatException {
        status = ConnectionStatus.disconnected;
        notifyListeners();
        rethrow;
      } catch (_) {
        if (status == ConnectionStatus.revoked) return;
        rethrow;
      }
    }
    status = ConnectionStatus.disconnected;
    notifyListeners();
  }
}
