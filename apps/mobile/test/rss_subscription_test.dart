// ignore_for_file: deprecated_member_use
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/app/app.dart';
import 'package:shared_preferences/shared_preferences.dart';

const feedUrl = 'https://podcast.example/feed.xml';
const oldUrl = 'https://old.example/feed.xml';
Completer<void>? slowFeed;
const audioUrl = 'https://podcast.example/audio.mp3';
const feed =
    '<rss><channel><title>Show thật</title><description>Mô tả</description><item><title>Tập thật</title><enclosure url="$audioUrl"/></item></channel></rss>';

class FixtureClient implements HttpClient {
  @override
  Future<HttpClientRequest> getUrl(Uri url) async {
    if (url.toString() == oldUrl) {
      await slowFeed?.future;
      return FixtureRequest(feed.replaceFirst('Show thật', 'Show cũ'));
    }
    if (url.toString() != feedUrl) throw const SocketException('offline');
    return FixtureRequest(feed);
  }

  @override
  void close({bool force = false}) {}
  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class FixtureRequest implements HttpClientRequest {
  FixtureRequest(this.body);
  final String body;
  @override
  bool followRedirects = true;
  @override
  Future<HttpClientResponse> close() async => FixtureResponse(body);
  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class FixtureResponse extends StreamView<List<int>>
    implements HttpClientResponse {
  FixtureResponse(String body) : super(Stream.value(utf8.encode(body)));
  @override
  int get statusCode => 200;
  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  testWidgets(
    'unpaired user adds HTTPS RSS, plays enclosure and restores subscription',
    (tester) async {
      SharedPreferences.setMockInitialValues({});
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(
        const MethodChannel('plugins.it_nomads.com/flutter_secure_storage'),
        (_) async => null,
      );
      MethodCall? opened;
      messenger.setMockMethodCallHandler(
        const MethodChannel('tube2pod/home_transport'),
        (call) async {
          if (call.method == 'open') {
            opened = call;
          }
          if (call.method == 'position') {
            return {'position': 0, 'duration': 1000, 'playing': true};
          }
          return null;
        },
      );
      await HttpOverrides.runZoned(() async {
        await tester.pumpWidget(const Tube2PodApp());
        await tester.pumpAndSettle();
        await tester.tap(find.text('Show').first);
        await tester.pumpAndSettle();
        await tester.tap(find.text('Dán link RSS'));
        await tester.pumpAndSettle();
        await tester.enterText(
          find.byType(TextField),
          'http://podcast.example/feed.xml',
        );
        await tester.tap(find.text('Thêm show'));
        await tester.pumpAndSettle();
        expect(find.text('Nhập link RSS HTTPS hợp lệ'), findsOneWidget);
        await tester.enterText(
          find.byType(TextField),
          'https://offline.example/feed.xml',
        );
        await tester.tap(find.text('Thêm show'));
        await tester.pumpAndSettle();
        expect(find.text('Không tải được feed RSS'), findsOneWidget);
        expect(
          (await SharedPreferences.getInstance()).getStringList(
            'subscribed_feed_urls',
          ),
          isNull,
        );
        await tester.enterText(find.byType(TextField), feedUrl);
        await tester.tap(find.text('Thêm show'));
        await tester.pumpAndSettle();
        expect(find.text('Show thật'), findsOneWidget);
        expect(
          (await SharedPreferences.getInstance()).getStringList(
            'subscribed_feed_urls',
          ),
          [feedUrl],
        );
        await tester.tap(find.text('Dán link RSS'));
        await tester.pumpAndSettle();
        await tester.enterText(find.byType(TextField), feedUrl);
        await tester.tap(find.text('Thêm show'));
        await tester.pumpAndSettle();
        expect(find.text('Show đã được thêm'), findsOneWidget);
        await tester.tap(find.text('Hủy'));
        await tester.pumpAndSettle();
        await tester.tap(find.text('Show thật'));
        await tester.pumpAndSettle();
        expect(find.text('Tập thật'), findsOneWidget);
        await tester.tap(find.byTooltip('Phát').first);
        await tester.pump();
        expect((opened!.arguments as Map)['uri'], audioUrl);
        expect((opened!.arguments as Map)['serverKey'], isNull);
        expect((opened!.arguments as Map)['headers'], isEmpty);
        await tester.pumpWidget(const SizedBox.shrink());
        await tester.pumpWidget(const Tube2PodApp());
        await tester.pumpAndSettle();
        await tester.tap(find.text('Show').first);
        await tester.pumpAndSettle();
        expect(find.text('Show thật'), findsOneWidget);
        await tester.tap(find.text('Show thật'));
        await tester.pumpAndSettle();
        await tester.tap(find.text('Bỏ theo dõi'));
        await tester.pumpAndSettle();
        expect(
          (await SharedPreferences.getInstance()).getStringList(
            'subscribed_feed_urls',
          ),
          isEmpty,
        );
      }, createHttpClient: (_) => FixtureClient());
    },
  );
  testWidgets('adding while saved feed loads keeps both subscriptions', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({
      'subscribed_feed_urls': [oldUrl],
    });
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(
      const MethodChannel('plugins.it_nomads.com/flutter_secure_storage'),
      (_) async => null,
    );
    messenger.setMockMethodCallHandler(
      const MethodChannel('tube2pod/home_transport'),
      (_) async => null,
    );
    slowFeed = Completer<void>();
    await HttpOverrides.runZoned(() async {
      await tester.pumpWidget(const Tube2PodApp());
      await tester.pumpAndSettle();
      await tester.tap(find.text('Show').first);
      await tester.pumpAndSettle();
      await tester.tap(find.text('Dán link RSS'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField), feedUrl);
      await tester.tap(find.text('Thêm show'));
      await tester.pumpAndSettle();
      expect(find.text('Show thật'), findsOneWidget);
      slowFeed!.complete();
      await tester.pumpAndSettle();
      expect(find.text('Show cũ'), findsOneWidget);
      expect(find.text('Show thật'), findsOneWidget);
      expect(
        (await SharedPreferences.getInstance()).getStringList(
          'subscribed_feed_urls',
        ),
        [oldUrl, feedUrl],
      );
      await tester.pumpWidget(const SizedBox.shrink());
    }, createHttpClient: (_) => FixtureClient());
    slowFeed = null;
  });
  testWidgets('a stalled feed reports failure and does not save a URL', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(
      const MethodChannel('plugins.it_nomads.com/flutter_secure_storage'),
      (_) async => null,
    );
    messenger.setMockMethodCallHandler(
      const MethodChannel('tube2pod/home_transport'),
      (_) async => null,
    );
    slowFeed = Completer<void>();
    await HttpOverrides.runZoned(() async {
      await tester.pumpWidget(const Tube2PodApp());
      await tester.pumpAndSettle();
      await tester.tap(find.text('Show').first);
      await tester.pumpAndSettle();
      await tester.tap(find.text('Dán link RSS'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField), oldUrl);
      await tester.tap(find.text('Thêm show'));
      await tester.pump();
      await tester.pump(const Duration(seconds: 11));
      await tester.pumpAndSettle();
      expect(find.text('Không tải được feed RSS'), findsOneWidget);
      expect(
        (await SharedPreferences.getInstance()).getStringList(
          'subscribed_feed_urls',
        ),
        isNull,
      );
      slowFeed!.complete();
      await tester.pumpWidget(const SizedBox.shrink());
    }, createHttpClient: (_) => FixtureClient());
    slowFeed = null;
  });
}
