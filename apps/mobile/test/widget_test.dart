import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/platform/home_transport.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  test('API and paired player normalize QR key and pass credentials', () async {
    const channel = MethodChannel('tube2pod/home_transport');
    final methods = <String>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger.setMockMethodCallHandler(channel, (call) async {
      methods.add(call.method);
      final args = call.arguments as Map;
      expect(args['serverKey'], '+/8=');
      expect(args['headers'], {'Authorization': 'credential'});
      if (call.method == 'request') {
        expect(args['serverId'], 'home');
        return {'status': 200, 'body': Uint8List.fromList([1])};
      }
      return null;
    });
    final endpoint = HomeEndpoint(uri: Uri.parse('https://home/'), serverId: 'home', serverKey: '-_8');
    final response = await HomeTransport().request(endpoint, const HomeRequest(headers: {'Authorization': 'credential'}));
    await HomePlayer().open(HomeMediaSource.paired(endpoint, headers: const {'Authorization': 'credential'}));
    expect(response.status, 200);
    expect(response.body, [1]);
    expect(methods, ['request', 'open']);
  });
}
