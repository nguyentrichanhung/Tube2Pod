// ignore_for_file: deprecated_member_use
import 'package:flutter/material.dart';
import 'package:mobile_scanner/mobile_scanner.dart';

class Scanner extends StatefulWidget {
  const Scanner({super.key});
  @override
  State<Scanner> createState() => _ScannerState();
}

class _ScannerState extends State<Scanner> {
  bool found = false;
  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: const Text('Quét QR')),
    body: MobileScanner(
      errorBuilder: (context, error) => const Center(
        child: Padding(
          padding: EdgeInsets.all(24),
          child: Text(
            'Không mở được camera. Hãy cấp quyền camera trong cài đặt rồi thử lại.',
          ),
        ),
      ),
      onDetect: (capture) {
        if (!found && capture.barcodes.firstOrNull?.rawValue != null) {
          found = true;
          Navigator.pop(context, capture.barcodes.first.rawValue);
        }
      },
    ),
  );
}
