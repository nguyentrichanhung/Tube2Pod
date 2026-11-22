// ignore_for_file: deprecated_member_use
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:mobile/app/app.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:mobile/features/connection/connection_controller.dart';
import 'package:mobile/platform/home_transport.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets('home navigation and local theme selection', (tester) async {
    await tester.pumpWidget(const Tube2PodApp());
    await tester.pumpAndSettle();
    expect(find.text('Nghe podcast'), findsOneWidget);
    await tester.tap(find.text('Quản trị'));
    await tester.pumpAndSettle();
    expect(find.text('Mở Kết nối'), findsOneWidget);
    await tester.tap(find.byTooltip('Giao diện'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Tối').last);
    await tester.pumpAndSettle();
    expect(find.text('Mở Kết nối'), findsOneWidget);
    expect(
      tester.widget<MaterialApp>(find.byType(MaterialApp)).themeMode,
      ThemeMode.dark,
    );
  });
  const uri = String.fromEnvironment('HOME_URI');
  const key = String.fromEnvironment('HOME_KEY');
  const id = String.fromEnvironment('HOME_ID');
  const ticket = String.fromEnvironment('HOME_TICKET');
  testWidgets(
    'real LAN pair persists credential, reads show and plays authorized audio',
    (tester) async {
      const storage = FlutterSecureStorage();
      final connection = ConnectionController(
        storage: storage,
        discover: () async => [],
      );
      await connection.pair(PairEnvelopeV1(id, Uri.parse(uri), key, ticket));
      expect(connection.status, ConnectionStatus.lan);
      expect(await storage.read(key: 'home'), contains('credential'));
      final shows = await connection.request('/v1/shows') as List;
      expect(shows, isNotEmpty);
      final episodes = await connection.request(
        '/v1/shows/${shows.first['id']}/episodes?status=published',
      ) as List;
      expect(episodes, isNotEmpty);
      final player = HomePlayer();
      await player.open(
        HomeMediaSource.paired(
          connection.endpoint(episodes.first['media_path'] as String),
          headers: connection.headers,
        ),
      );
      final before = (await player.position())!;
      expect(before['playing'], true);
      final duration = before['duration'] as int;
      expect(duration, greaterThan(0));
      await player.seek(Duration(milliseconds: duration ~/ 2));
      await Future<void>.delayed(const Duration(seconds: 1));
      expect(
        (await player.position())!['position'] as int,
        greaterThanOrEqualTo(duration ~/ 2),
      );
      await player.close();
      final restored = ConnectionController(
        storage: storage,
        discover: () async => [],
      );
      await restored.reconnect();
      expect(restored.status, ConnectionStatus.lan);
      await storage.delete(key: 'home');
    },
    skip: uri.isEmpty,
    timeout: const Timeout(Duration(minutes: 2)),
  );
}
