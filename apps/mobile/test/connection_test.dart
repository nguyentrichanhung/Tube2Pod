import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:mobile/features/connection/connection_controller.dart';
import 'package:mobile/platform/home_transport.dart';

class MemoryStorage extends FlutterSecureStorage {
  String? saved;
  @override
  Future<void> write({
    required String key,
    required String? value,
    AppleOptions? iOptions,
    AndroidOptions? aOptions,
    LinuxOptions? lOptions,
    WebOptions? webOptions,
    AppleOptions? mOptions,
    WindowsOptions? wOptions,
  }) async {
    saved = value;
  }

  @override
  Future<String?> read({
    required String key,
    AppleOptions? iOptions,
    AndroidOptions? aOptions,
    LinuxOptions? lOptions,
    WebOptions? webOptions,
    AppleOptions? mOptions,
    WindowsOptions? wOptions,
  }) async => saved;
}

class FakeTransport extends HomeTransport {
  String returnedId = 'home';
  bool mismatch = false;
  final paths = <String>[];
  @override
  Future<HomeResponse> request(
    HomeEndpoint endpoint,
    HomeRequest request,
  ) async {
    paths.add(endpoint.uri.toString());
    if (mismatch) throw PlatformException(code: 'identityMismatch');
    final body = endpoint.uri.path == '/v1/pair'
        ? {'server_id': returnedId, 'credential': 'secret'}
        : {'server_id': returnedId, 'admin_uri': null, 'public_origin': null};
    return HomeResponse(200, Uint8List.fromList(utf8.encode(jsonEncode(body))));
  }
}

void main() {
  test('rejects expired or invalid pairing envelopes', () {
    expect(
      () => PairEnvelopeV1.parse(
        '{"version":1,"server_id":"home","server_key":"AQ","ticket":"x","lan_uri":"https://home","expires_at":1}',
      ),
      throwsFormatException,
    );
    expect(
      () => PairEnvelopeV1.parse(
        '{"version":1,"server_id":"home","server_key":"AQ","ticket":"x","lan_uri":"http://home","expires_at":9999999999}',
      ),
      throwsFormatException,
    );
  });
  test('pairs with pinned endpoint and checks server identity before storing credential', () async {
    final transport = FakeTransport();
    final storage = MemoryStorage();
    final controller = ConnectionController(
      transport: transport,
      storage: storage,
    );
    final envelope = PairEnvelopeV1(
      'home',
      Uri.parse('https://192.168.1.5:1234'),
      'AQ',
      'ticket',
    );
    transport.returnedId = 'other';
    await expectLater(controller.pair(envelope), throwsStateError);
    expect(storage.saved, isNull);
    transport.returnedId = 'home';
    await controller.pair(envelope);
    expect(controller.status, ConnectionStatus.lan);
    expect(storage.saved, contains('secret'));
    expect(transport.paths.map((p) => Uri.parse(p).path), [
      '/v1/pair',
      '/v1/pair',
      '/v1/connection',
    ]);
  });
  test(
    'reconnect stops on pin mismatch and saves verified changed address',
    () async {
      final transport = FakeTransport();
      final storage = MemoryStorage();
      final controller = ConnectionController(
        transport: transport,
        storage: storage,
        discover: () async => [Uri.parse('https://192.168.1.6:1234')],
      );
      await controller.pair(
        PairEnvelopeV1(
          'home',
          Uri.parse('https://192.168.1.5:1234'),
          'AQ',
          'ticket',
        ),
      );
      transport.mismatch = true;
      await expectLater(
        controller.reconnect(),
        throwsA(isA<PlatformException>()),
      );
      expect(transport.paths.last, 'https://192.168.1.6:1234/v1/connection');
      expect(transport.paths.where((p) => p.contains('192.168.1.5')).length, 2);
      transport.mismatch = false;
      await controller.reconnect();
      expect(controller.status, ConnectionStatus.lan);
      expect(storage.saved, contains('192.168.1.6'));
    },
  );
}
