// ignore_for_file: deprecated_member_use
import 'package:flutter/material.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:mobile/app/app.dart';
import 'package:mobile/features/connection/connection_controller.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  const uri = String.fromEnvironment('HOME_URI');
  const showTitle = String.fromEnvironment('HOME_SHOW');
  const episodeTitle = String.fromEnvironment('HOME_EPISODE');

  testWidgets(
    'paired app publishes a ready episode and creates a show',
    (tester) async {
      const storage = FlutterSecureStorage();
      await storage.delete(key: 'home');
      final connection = ConnectionController(
        storage: storage,
        discover: () async => [],
      );
      await connection.pair(
        PairEnvelopeV1(
          const String.fromEnvironment('HOME_ID'),
          Uri.parse(uri),
          const String.fromEnvironment('HOME_KEY'),
          const String.fromEnvironment('HOME_TICKET'),
        ),
      );
      addTearDown(() => storage.delete(key: 'home'));
      final shows = await connection.request('/v1/shows') as List;
      final show = shows.singleWhere((show) => show['title'] == showTitle);
      await tester.pumpWidget(const Tube2PodApp());

      Future<void> waitFor(Finder finder) async {
        for (var i = 0; i < 35; i++) {
          await tester.pump();
          if (finder.evaluate().isNotEmpty) return;
          await Future<void>.delayed(const Duration(milliseconds: 200));
        }
        expect(finder, findsWidgets);
      }

      await tester.tap(find.text('Quản trị'));
      await waitFor(find.text(showTitle));
      await tester.tap(find.text(showTitle).first);
      await waitFor(find.text(episodeTitle));
      await tester.tap(find.text('Phát hành').last);
      await waitFor(find.byType(AlertDialog));
      await tester.tap(
        find.descendant(
          of: find.byType(AlertDialog),
          matching: find.text('Phát hành'),
        ),
      );
      await waitFor(find.text('Chưa có tập sẵn sàng'));
      final published = await connection.request(
        '/v1/shows/${show['id']}/episodes?status=published',
      ) as List;
      expect(
        published.any((episode) => episode['title'] == episodeTitle),
        isTrue,
      );

      await tester.pageBack();
      await waitFor(find.text('Tạo show'));
      await tester.tap(find.text('Tạo show').last);
      await waitFor(find.byType(TextField));
      await tester.enterText(find.byType(TextField).at(0), 'Show tạo bằng app');
      await tester.enterText(find.byType(TextField).at(1), 'Mô tả thử');
      await tester.ensureVisible(find.text('Lưu').last);
      await tester.tap(find.text('Lưu').last);
      await waitFor(find.text('Show tạo bằng app'));
      final updatedShows = await connection.request('/v1/shows') as List;
      expect(
        updatedShows.any((show) => show['title'] == 'Show tạo bằng app'),
        isTrue,
      );
    },
    skip: uri.isEmpty || showTitle.isEmpty || episodeTitle.isEmpty,
    timeout: const Timeout(Duration(minutes: 2)),
  );
}
