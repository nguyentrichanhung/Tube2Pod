import 'dart:convert';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:mobile/platform/home_transport.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  const host = String.fromEnvironment('FIXTURE_HOST', defaultValue: '10.0.2.2');
  const port = String.fromEnvironment('FIXTURE_PORT');
  const relay = String.fromEnvironment('FIXTURE_RELAY');
  const pin = String.fromEnvironment('FIXTURE_PIN');
  const wrongRelay = String.fromEnvironment('WRONG_RELAY');
  const wrongPin = String.fromEnvironment('WRONG_PIN');
  const enclosure = String.fromEnvironment('ENCLOSURE_URL');
  HomeEndpoint endpoint(String port, String path, [String key = pin]) => HomeEndpoint(
    uri: Uri.parse('https://$host:$port$path'), serverId: 'fixture', serverKey: key,
  );
  final transport = HomeTransport();
  testWidgets('direct and relayed API preserve identity before credentials', (tester) async {
    expect(pin, isNotEmpty, reason: 'start tests/native_transport_fixture.py and supply FIXTURE_*');
    for (final currentPort in [port, relay]) {
      final response = await transport.request(endpoint(currentPort, '/'),
        const HomeRequest(headers: {'Authorization': 'fixture-secret'}));
      expect(utf8.decode(response.body), 'fixture api');
    }
    await expectLater(
      transport.request(endpoint(wrongRelay, '/'), const HomeRequest(headers: {'Authorization': 'must-not-arrive'})),
      throwsA(isA<PlatformException>().having((e) => e.code, 'code', 'identityMismatch')),
    );
    final received = await transport.request(endpoint(wrongRelay, '/received', wrongPin), const HomeRequest());
    expect(utf8.decode(received.body), isNot(contains('must-not-arrive')));
  });
  testWidgets('public HTTPS enclosure plays and seeks', (tester) async {
    final player = HomePlayer();
    await player.open(HomeMediaSource.enclosure(Uri.parse(enclosure)));
    final initial = (await player.position())!;
    expect(initial['playing'], true);
    final duration = initial['duration'] as int;
    expect(duration, greaterThan(0));
    await player.seek(Duration(milliseconds: duration ~/ 2));
    await Future<void>.delayed(const Duration(seconds: 1));
    expect((await player.position())!['position'] as int, greaterThanOrEqualTo(duration ~/ 2));
    await player.close();
  }, skip: enclosure.isEmpty);
  testWidgets('rejects incorrect media ranges and truncated bodies', (tester) async {
    for (final path in ['/audio-bad-range', '/audio-short']) {
      final player = HomePlayer();
      await expectLater(
        player.open(HomeMediaSource.paired(endpoint(port, path), headers: const {'Authorization': 'fixture-media-secret'})),
        throwsA(isA<PlatformException>()),
      );
      await player.close();
    }
  });
  testWidgets('paired MP3 plays and seeks through verified range requests', (tester) async {
    final player = HomePlayer();
    await player.open(HomeMediaSource.paired(endpoint(port, '/audio'), headers: {'Authorization': 'fixture-media-secret'}));
    final initial = (await player.position())!;
    expect(initial['playing'], true);
    final duration = initial['duration'] as int;
    expect(duration, greaterThan(0));
    await player.seek(Duration(milliseconds: duration ~/ 2));
    await Future<void>.delayed(const Duration(seconds: 1));
    final after = (await player.position())!;
    expect(after['position'] as int, greaterThanOrEqualTo(duration ~/ 2));
    await player.close();
    final received = await transport.request(endpoint(port, '/media-requests'), const HomeRequest());
    final requests = jsonDecode(utf8.decode(received.body)) as List;
    expect(requests.any((entry) => entry[0] == 'HEAD' && entry[1] == 'fixture-media-secret'), isTrue);
    expect(requests.any((entry) => entry[0] == 'GET' && entry[1] == 'fixture-media-secret'), isTrue);
  });
}
