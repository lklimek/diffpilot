#!/usr/bin/perl
# Test doubles for deploy/egress-proxy/test.sh (core Perl only; runs in the proxy image).
#   dns NAME=IP[,IP...] ...   UDP DNS on port 53: A answers for the listed names, empty answers for their other
#                             types; every other query is relayed to Docker's resolver 127.0.0.11.
#   http PORT LABEL           HTTP on PORT: answers "LABEL <request line>" in the body; a path containing
#                             /garbage gets a non-HTTP answer.
use strict;
use warnings;
use IO::Socket::INET;

$| = 1;
my $mode = shift @ARGV // die "usage: dns NAME=IP... | http PORT LABEL\n";

if ($mode eq 'dns') {
  my %answers;
  for my $spec (@ARGV) {
    my ($name, $ips) = split /=/, $spec, 2;
    $answers{lc $name} = [split /,/, $ips];
  }
  my $sock = IO::Socket::INET->new(LocalPort => 53, Proto => 'udp') or die "dns: bind: $!\n";
  my $upstream = IO::Socket::INET->new(PeerAddr => '127.0.0.11:53', Proto => 'udp') or die "dns: upstream: $!\n";
  while (1) {
    my $peer = $sock->recv(my $query, 512) // next;
    next if length $query < 17;
    my ($pos, @labels) = (12);
    while ((my $len = ord substr($query, $pos, 1)) > 0) {
      push @labels, substr($query, $pos + 1, $len);
      $pos += $len + 1;
    }
    my $qtype = unpack 'n', substr($query, $pos + 1, 2);
    my $question = substr($query, 12, $pos + 5 - 12);
    my $ips = $answers{lc join '.', @labels};
    if (!$ips) {
      $upstream->send($query);
      my $ready = '';
      vec($ready, fileno $upstream, 1) = 1;
      next unless select($ready, undef, undef, 3);
      $upstream->recv(my $reply, 4096);
      $sock->send($reply, 0, $peer);
      next;
    }
    my @records = $qtype == 1 ? @$ips : ();
    my $reply = substr($query, 0, 2) . pack('nnnnn', 0x8180, 1, scalar @records, 0, 0) . $question;
    $reply .= pack('nnnNn', 0xc00c, 1, 1, 5, 4) . pack('C4', split /\./) for @records;
    $sock->send($reply, 0, $peer);
  }
}

if ($mode eq 'http') {
  my ($port, $label) = @ARGV;
  my $server = IO::Socket::INET->new(LocalPort => $port, Listen => 16, ReuseAddr => 1) or die "http: bind: $!\n";
  while (my $client = $server->accept) {
    # One client at a time; a silent one (a tunnel) is dropped after 5 s.
    eval {
      local $SIG{ALRM} = sub { die "timeout\n" };
      alarm 5;
      my $line = <$client> // die "empty\n";
      while (my $header = <$client>) { last if $header =~ /^\r?\n$/ }
      $line =~ s/\r?\n$//;
      print "request $line\n";
      if ($line =~ m{/garbage}) {
        print $client "NOT HTTP AT ALL\r\n\r\n";
      } else {
        my $body = "$label $line\n";
        print $client "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: " . length($body)
          . "\r\nConnection: close\r\n\r\n$body";
      }
      alarm 0;
    };
    print "connection without a request ($@)" if $@;
    close $client;
  }
}
die "unknown mode $mode\n";
